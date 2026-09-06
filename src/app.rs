use std::{
    collections::{BTreeMap, VecDeque, hash_map::DefaultHasher},
    fs,
    future::Future,
    hash::{Hash, Hasher},
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, mpsc, mpsc::Receiver},
    task::{Context as TaskContext, Poll, Wake, Waker},
    thread,
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align, Color32, ColorImage, KeyboardShortcut, Layout, Modifiers, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2,
    text::{CCursor, CCursorRange},
};
use egui_ltreeview::{Action as TreeAction, NodeBuilder, TreeView, TreeViewBuilder, TreeViewState};
use rfd::{AsyncFileDialog, FileHandle};

use crate::{
    asset::{AssetLoader, LoadedAsset},
    builtin_themes,
    compiler::{CompileRequest, Compiler, PreviewLink, PreviewPage},
    diagnostics::{
        Diagnostic, DiagnosticLocation, DiagnosticSeverity, DiagnosticSource,
        normalize_diagnostics, parse_typst_short_output,
    },
    document::DocumentKind,
    generic_highlight::GenericSyntaxHighlighter,
    highlight::SyntaxHighlighter,
    lsp_text::{
        apply_text_edits, lsp_position_at_char, range_to_char_range, scalar_position_at_char,
    },
    native_menu::{
        ApplicationCommand, EditCommand, FileCommand, NativeMenuCommand, NativeMenuCommandQueue,
        NativeMenuReceiver, ViewCommand,
    },
    open_requests::OpenRequestReceiver,
    preview::{
        PAGE_MARGIN, PDF_POINTS_PER_PREVIEW_PIXEL, dark_preview_rgba, page_stack_geometry,
        stack_height, visible_page, zoom_anchored_offset,
    },
    project_index::{ProjectIndex, analyze_project},
    screenshot::{CaptureController, CaptureThemeProfile, UiSnapshotScene},
    search::{SearchState, find_all},
    settings::{
        AppSettings, ColorThemeChoice, DEFAULT_HOVER_DELAY_MS, DEFAULT_HOVER_FADE_MS,
        DocumentTheme, InterfaceTheme, PreviewPreference, SYSTEM_THEME_ID, SourcePreviewTrigger,
        ToolMode, ToolPreference,
    },
    sublime_theme::{self, ImportedTheme, Rgba, ThemeFormat},
    syntax_theme::{
        ResolvedTypstStyles, TypstOverrideThemes, TypstStyleOverride, TypstStyleOverrides,
        TypstSyntaxRole,
    },
    theme::{self, METRICS},
    theme_transform::ThemeTransform,
    tinymist::{
        DiagnosticSeverity as TinymistDiagnosticSeverity, Generation, InvertColors, LspRange,
        LspTextEdit, TextDocument, TinymistConfig, TinymistDiagnostic, TinymistEvent,
        TinymistSidecar, UnsavedTextDocument,
    },
    toolchain::{ToolKind, ToolOrigin, ToolResolution, resolve_tool},
    workspace::{WorkspaceNode, WorkspaceSnapshot, WorkspaceTree},
};

const COMPILE_DEBOUNCE: Duration = Duration::from_millis(60);
const WORKSPACE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const AUTOSAVE_RETRY_DELAY: Duration = Duration::from_secs(2);
const PROJECT_INDEX_DEBOUNCE: Duration = Duration::from_millis(180);
const MIN_PREVIEW_ZOOM: f32 = 0.2;
const MAX_PREVIEW_ZOOM: f32 = 6.0;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ThemeSourceRequest {
    Builtin(String),
    Sublime(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveThemeRequest {
    source: ThemeSourceRequest,
    invert: bool,
    hue_shift_degrees: i16,
    fallback_dark: bool,
}

impl ActiveThemeRequest {
    fn transform(&self) -> ThemeTransform {
        ThemeTransform::new(self.invert, f32::from(self.hue_shift_degrees))
    }
}

fn active_theme_request(
    settings: &AppSettings,
    system_theme: Option<egui::Theme>,
    launch_override: Option<&CaptureThemeProfile>,
) -> ActiveThemeRequest {
    let system_dark = system_theme.unwrap_or(egui::Theme::Dark) == egui::Theme::Dark;
    if let Some(profile) = launch_override {
        let id = if profile.name == SYSTEM_THEME_ID {
            paired_tiptop_theme_id(system_dark)
        } else {
            profile.name.as_str()
        };
        return ActiveThemeRequest {
            source: ThemeSourceRequest::Builtin(id.to_owned()),
            invert: profile.invert,
            hue_shift_degrees: profile.hue_shift_degrees,
            fallback_dark: system_dark,
        };
    }

    let preferred_theme = settings
        .interface_theme
        .resolve(system_theme, egui::Theme::Dark);
    let fallback_dark = preferred_theme == egui::Theme::Dark;
    let source = match settings.color_theme(preferred_theme) {
        ColorThemeChoice::Builtin(id) => ThemeSourceRequest::Builtin(id.clone()),
        ColorThemeChoice::Sublime(path) => ThemeSourceRequest::Sublime(PathBuf::from(path)),
    };
    ActiveThemeRequest {
        source,
        invert: settings.theme_invert,
        hue_shift_degrees: settings.theme_hue_shift_degrees,
        fallback_dark,
    }
}

fn theme_request_for_appearance(settings: &AppSettings, dark: bool) -> ActiveThemeRequest {
    let appearance = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    let source = match settings.color_theme(appearance) {
        ColorThemeChoice::Builtin(id) => ThemeSourceRequest::Builtin(id.clone()),
        ColorThemeChoice::Sublime(path) => ThemeSourceRequest::Sublime(PathBuf::from(path)),
    };
    ActiveThemeRequest {
        source,
        invert: settings.theme_invert,
        hue_shift_degrees: settings.theme_hue_shift_degrees,
        fallback_dark: dark,
    }
}

fn tinymist_invert_colors(document_theme: DocumentTheme, preview_dark: bool) -> InvertColors {
    match document_theme {
        DocumentTheme::FollowInterface => {
            if preview_dark {
                InvertColors::Always
            } else {
                InvertColors::Never
            }
        }
        DocumentTheme::Light => InvertColors::Never,
        DocumentTheme::Dark => InvertColors::Always,
    }
}

fn tinymist_restart_required(
    document_theme_mode_changed: bool,
    preview_appearance_changed: bool,
    preview_preference_changed: bool,
    tinymist_program_changed: bool,
    refresh_tools: bool,
) -> bool {
    document_theme_mode_changed
        || preview_appearance_changed
        || preview_preference_changed
        || tinymist_program_changed
        || refresh_tools
}

fn may_create_embedded_webview(prevent_background_activation: bool, focused: Option<bool>) -> bool {
    !prevent_background_activation || focused != Some(false)
}

fn paired_tiptop_theme_id(dark_mode: bool) -> &'static str {
    builtin_themes::default_for_mode(dark_mode).id
}

fn active_theme_preference(
    interface_theme: InterfaceTheme,
    has_launch_override: bool,
    active_dark: bool,
) -> egui::ThemePreference {
    if !has_launch_override && interface_theme == InterfaceTheme::System {
        // Leaving egui in system mode is important on macOS: a fixed preference
        // becomes a per-window AppKit appearance override, which prevents winit
        // from receiving later system appearance changes.
        egui::ThemePreference::System
    } else if active_dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    }
}

fn load_active_theme(request: &ActiveThemeRequest) -> Result<ImportedTheme, String> {
    let mut imported = match &request.source {
        ThemeSourceRequest::Builtin(id) => {
            let builtin =
                builtin_themes::find(id).ok_or_else(|| format!("Unknown built-in theme {id:?}"))?;
            ImportedTheme {
                name: Some(builtin.name.to_owned()),
                author: Some("tiptoptyp".to_owned()),
                format: ThemeFormat::Builtin,
                dark_mode: builtin.dark_mode,
                palette: builtin.palette,
                syntect_theme: builtin.syntect_theme(),
            }
        }
        ThemeSourceRequest::Sublime(path) => {
            let imported = sublime_theme::import_path(path).map_err(|error| error.to_string())?;
            if imported.dark_mode != request.fallback_dark {
                let inferred = if imported.dark_mode { "dark" } else { "light" };
                let assigned = if request.fallback_dark {
                    "dark"
                } else {
                    "light"
                };
                return Err(format!(
                    "{} is a {inferred} Sublime theme but is assigned to the {assigned} slot",
                    path.display()
                ));
            }
            imported
        }
    };
    request.transform().apply_imported_theme(&mut imported);
    Ok(imported)
}

fn load_active_theme_or_fallback(request: &ActiveThemeRequest) -> (ImportedTheme, Option<String>) {
    match load_active_theme(request) {
        Ok(theme) => (theme, None),
        Err(error) => {
            let fallback = ActiveThemeRequest {
                source: ThemeSourceRequest::Builtin(
                    paired_tiptop_theme_id(request.fallback_dark).to_owned(),
                ),
                invert: request.invert,
                hue_shift_degrees: request.hue_shift_degrees,
                fallback_dark: request.fallback_dark,
            };
            let theme = load_active_theme(&fallback)
                .expect("the paired built-in Tiptop fallback is always present");
            (theme, Some(error))
        }
    }
}

const DEFAULT_SOURCE: &str = r##"#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")

= Welcome to tiptoptyp

Edit this document on the left. The PDF preview updates as you type.

#let accent = rgb("#4f8cff")
#text(fill: accent, weight: "bold")[A small, fast Typst workspace.]

== Math and code

Inline math is highlighted too: $ integral_0^infinity e^(-x) dif x = 1 $.

#for item in ("Native UI", "Live PDF", "Fast rebuilds") [
  - #item
]
"##;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Code,
    Split,
    Preview,
}

impl ViewMode {
    fn shows_code(self) -> bool {
        matches!(self, Self::Code | Self::Split)
    }

    fn shows_preview(self) -> bool {
        matches!(self, Self::Split | Self::Preview)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewStatus {
    Waiting,
    Compiling,
    Ready(Duration),
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiIcon {
    Check,
    Close,
    Down,
    FitWidth,
    Next,
    Previous,
    Refresh,
    Eye,
    Up,
    Warning,
    Waiting,
    ZoomIn,
    ZoomOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveIntent {
    Explicit,
    ExplicitConfirmed,
    Auto,
}

#[derive(Debug, Clone)]
struct Notice {
    message: String,
    kind: NoticeKind,
}

#[derive(Debug, Clone)]
struct StatusLogEntry {
    detail: String,
    kind: NoticeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServiceState {
    Disabled(String),
    Starting(String),
    Ready(String),
    Degraded(String),
    Failed(String),
    Unsupported(String),
}

impl ServiceState {
    fn label(&self) -> &'static str {
        match self {
            Self::Disabled(_) => "Disabled",
            Self::Starting(_) => "Starting",
            Self::Ready(_) => "Ready",
            Self::Degraded(_) => "Degraded",
            Self::Failed(_) => "Failed",
            Self::Unsupported(_) => "Unsupported",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Disabled(detail)
            | Self::Starting(detail)
            | Self::Ready(detail)
            | Self::Degraded(detail)
            | Self::Failed(detail)
            | Self::Unsupported(detail) => detail,
        }
    }

    fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

struct PreviewTexture {
    /// Logical layout size. Images may deliberately differ from PDF raster
    /// pixels so one image pixel maps to one UI point at 100%.
    size: [usize; 2],
    raster_size: [usize; 2],
    rgba: Vec<u8>,
    links: Vec<PreviewLink>,
    texture: TextureHandle,
}

struct EguiFutureWake(egui::Context);

impl Wake for EguiFutureWake {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.request_repaint();
    }
}

#[derive(Clone)]
struct LineDiagnostic {
    line: usize,
    severity: DiagnosticSeverity,
    summary: String,
    detail: String,
}

#[derive(Debug, Clone, Copy)]
struct EditorAttention {
    char_index: usize,
    started: Instant,
}

#[derive(Debug, Clone)]
struct EditorHoverState {
    range: Range<usize>,
    request_token: u64,
    uri: String,
    version: i32,
    requested: bool,
    detail: Option<String>,
}

#[derive(Debug, Clone)]
struct DiagnosticTooltipOverlay {
    origin: Rect,
    anchor: Pos2,
    severity: DiagnosticSeverity,
    detail: String,
    opacity: f32,
}

#[derive(Debug, Clone)]
struct HoverTooltipOverlay {
    origin: Rect,
    anchor: Pos2,
    detail: String,
    opacity: f32,
}

#[derive(Debug, Clone, Copy)]
struct TooltipGeometry {
    origin: Rect,
    card: Rect,
    pointer_inside_card: bool,
}

#[derive(Debug, Clone, Copy)]
struct HoverRuntimeConfig {
    delay: Duration,
    fade: Duration,
}

#[derive(Debug, Clone, Copy)]
struct HoverTimingState {
    widget: egui::Id,
    started: f64,
    last_seen: f64,
}

#[derive(Debug, Clone)]
struct EditorSnapshot {
    source: String,
    cursor: CCursorRange,
}

struct PendingExport {
    path: PathBuf,
    document_epoch: u64,
}

struct PendingExportDialog {
    document_epoch: u64,
    future: Pin<Box<dyn Future<Output = Option<FileHandle>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPickerTarget {
    Typst,
    Tinymist,
    SublimeTheme { dark_mode: bool },
}

struct PendingToolPicker {
    target: ToolPickerTarget,
    future: Pin<Box<dyn Future<Output = Option<FileHandle>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentDialogTarget {
    OpenFile,
    OpenFileInNewWindow,
    OpenFolder,
    SaveAs { typst: bool },
}

struct PendingDocumentDialog {
    target: DocumentDialogTarget,
    document_epoch: u64,
    revision: u64,
    future: Pin<Box<dyn Future<Output = Option<FileHandle>>>>,
}

#[derive(Debug, Clone)]
struct RenameDialog {
    path: PathBuf,
    name: String,
    focus: bool,
}

#[derive(Debug, Clone)]
enum WorkspaceMenuAction {
    Open(PathBuf),
    UseForPreview(PathBuf),
    Rename(PathBuf),
    CopyPath(PathBuf),
    Reveal(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorMenuAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileMenuAction {
    New,
    NewWindow,
    Open,
    OpenInNewWindow,
    ChangeWorkspaceRoot,
    Save,
    SaveAs,
    ExportPdf,
}

/// A document-window request emitted by one editor session and fulfilled by
/// the process-wide application shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditorWindowRequest {
    /// Create a clean untitled document in this workspace.
    New { workspace_root: PathBuf },
    /// Open a file or workspace in an independent session.
    Open(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorWindowHost {
    Root,
    Secondary,
}

impl EditorWindowHost {
    fn is_root(self) -> bool {
        self == Self::Root
    }
}

fn may_create_window_webview(
    host: EditorWindowHost,
    prevent_background_activation: bool,
    focused: Option<bool>,
) -> bool {
    match host {
        EditorWindowHost::Root => {
            may_create_embedded_webview(prevent_background_activation, focused)
        }
        // The active platform handle is only an exact match for an immediate
        // child viewport while egui reports that same viewport focused.
        EditorWindowHost::Secondary => focused == Some(true),
    }
}

#[derive(Debug, Clone)]
enum AppPopup {
    File {
        anchor: Pos2,
    },
    Edit {
        anchor: Pos2,
    },
    Workspace {
        anchor: Pos2,
        path: PathBuf,
        is_file: bool,
    },
    Editor {
        anchor: Pos2,
    },
    StatusLog {
        anchor: Pos2,
    },
}

#[derive(Debug, Clone)]
enum AppPopupAction {
    File(FileMenuAction),
    Editor(EditorMenuAction),
    Find(bool),
    Workspace(WorkspaceMenuAction),
}

#[derive(Debug, Clone)]
enum DeferredDocumentAction {
    New,
    OpenFileDialog,
    OpenFolderDialog,
    CloseWindow,
    LoadPath(PathBuf),
    OpenFolder(PathBuf),
    FollowFileLink {
        path: PathBuf,
        page: Option<usize>,
        source_position: Option<(usize, usize)>,
    },
    FollowTinymistLocation {
        path: PathBuf,
        selection: Option<LspRange>,
    },
    SaveThen(Box<PendingDocumentAction>),
    ForceSave {
        path: PathBuf,
        document_epoch: u64,
        revision: u64,
        expected_disk_fingerprint: Option<u64>,
        observed_disk_fingerprint: Option<u64>,
    },
}

#[derive(Debug, Clone)]
struct PendingDocumentAction {
    action: DeferredDocumentAction,
    document_epoch: u64,
    revision: u64,
    allow_discard: bool,
    description: String,
}

#[derive(Debug, Clone)]
enum AppModal {
    Alert {
        title: String,
        message: String,
        kind: NoticeKind,
    },
    Unsaved {
        message: String,
        pending: PendingDocumentAction,
    },
    Overwrite {
        message: String,
        path: PathBuf,
        document_epoch: u64,
        revision: u64,
        expected_disk_fingerprint: Option<u64>,
        observed_disk_fingerprint: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppModalChoice {
    Primary,
    Secondary,
    Cancel,
}

pub struct EditorApp {
    source: String,
    saved_source: String,
    path: Option<PathBuf>,
    document_epoch: u64,
    revision: u64,
    disk_fingerprint: Option<u64>,
    document_kind: DocumentKind,
    highlighter: SyntaxHighlighter,
    generic_highlighter: GenericSyntaxHighlighter,

    compiler: Compiler,
    asset_loader: AssetLoader,
    asset_token: u64,
    pending_asset_page: Option<usize>,
    compile_deadline: Option<Instant>,
    status: PreviewStatus,
    raw_diagnostics: String,
    diagnostics: Vec<Diagnostic>,
    tinymist_diagnostics: Vec<Diagnostic>,
    compiled_revision: Option<u64>,
    pdf: Option<Vec<u8>>,
    pages: Vec<PreviewTexture>,
    visible_page: usize,
    zoom: f32,
    fit_width: bool,
    requested_zoom: Option<f32>,
    requested_page: Option<usize>,
    preview_dark: bool,
    preview_was_visible: bool,

    view_mode: ViewMode,
    filesystem_visible: bool,
    problems_visible: bool,
    settings_visible: bool,
    typst_overrides_visible: bool,
    typst_overrides_dark: bool,
    settings: AppSettings,
    pending_settings: Option<AppSettings>,
    imported_theme: Option<ImportedTheme>,
    applied_theme_request: ActiveThemeRequest,
    applied_typst_overrides: TypstOverrideThemes,
    theme_override: Option<CaptureThemeProfile>,
    applied_document_theme: DocumentTheme,
    applied_preview_preference: PreviewPreference,
    applied_typst_preference: ToolPreference,
    applied_tinymist_preference: ToolPreference,
    tool_refresh_requested: bool,
    typst_tool: ToolResolution,
    tinymist_tool: ToolResolution,
    workspace_root: PathBuf,
    workspace_chooser_visible: bool,
    workspace: Option<WorkspaceTree>,
    workspace_error: Option<String>,
    workspace_scan: Option<Receiver<(u64, Result<WorkspaceSnapshot, String>)>>,
    workspace_scan_id: u64,
    next_workspace_refresh: Instant,
    project_index: ProjectIndex,
    project_index_deadline: Option<Instant>,
    project_index_result: Option<Receiver<(u64, ProjectIndex)>>,
    project_index_request_id: u64,
    captures: CaptureController,
    snapshot_scene: Option<UiSnapshotScene>,
    window_host: EditorWindowHost,
    pending_window_requests: VecDeque<EditorWindowRequest>,
    open_requests: OpenRequestReceiver,
    native_menu_commands: NativeMenuReceiver,
    queued_native_menu_commands: NativeMenuCommandQueue,
    queued_open_requests: VecDeque<PathBuf>,

    find_visible: bool,
    replace_visible: bool,
    find_query: String,
    replacement: String,
    search: SearchState,
    focus_find: bool,
    pending_editor_selection: Option<Range<usize>>,
    editor_attention: Option<EditorAttention>,
    editor_hover: Option<EditorHoverState>,
    next_editor_hover_token: u64,
    reset_editor_history: bool,
    editor_undo: Vec<EditorSnapshot>,
    editor_redo: Vec<EditorSnapshot>,
    autosave_deadline: Option<Instant>,
    diagnostic_tooltip: Option<DiagnosticTooltipOverlay>,
    app_popup: Option<AppPopup>,
    app_popup_had_focus: bool,
    pending_app_popup_action: Option<AppPopupAction>,
    app_modal: Option<AppModal>,
    app_modal_had_focus: bool,
    app_modal_suspended: bool,
    pending_document_action: Option<PendingDocumentAction>,
    post_save_action: Option<PendingDocumentAction>,
    rename_dialog: Option<RenameDialog>,
    rename_overlay_had_focus: bool,
    rename_overlay_suspended: bool,

    pending_export: Option<PendingExport>,
    pending_export_dialog: Option<PendingExportDialog>,
    pending_tool_picker: Option<PendingToolPicker>,
    pending_document_dialog: Option<PendingDocumentDialog>,
    notice: Option<Notice>,
    status_log: VecDeque<StatusLogEntry>,
    recorded_status: Option<PreviewStatus>,
    last_title: String,
    allow_close: bool,

    tinymist: TinymistSidecar,
    tinymist_generation: Option<Generation>,
    tinymist_uri: Option<String>,
    tinymist_preview_uri: Option<String>,
    tinymist_current_open: bool,
    tinymist_unsaved_document: Option<UnsavedTextDocument>,
    tinymist_url: Option<String>,
    tinymist_preview_enabled: bool,
    tinymist_lsp_ready: bool,
    tinymist_state: ServiceState,
    webview_state: ServiceState,

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    native_window_parent: Option<crate::native_window::ActiveWindowHandle>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview: Option<wry::WebView>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview_url: Option<String>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    web_link_sender: mpsc::Sender<String>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    web_link_receiver: mpsc::Receiver<String>,
}

impl EditorApp {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        captures: CaptureController,
        theme_override: Option<CaptureThemeProfile>,
        snapshot_scene: Option<UiSnapshotScene>,
        open_requests: OpenRequestReceiver,
        native_menu_commands: NativeMenuReceiver,
    ) -> Self {
        Self::new_session(
            &context.egui_ctx,
            initial_path,
            captures,
            theme_override,
            snapshot_scene,
            open_requests,
            native_menu_commands,
            AppSettings::load(context.storage),
            EditorWindowHost::Root,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_session(
        context: &egui::Context,
        initial_path: Option<PathBuf>,
        captures: CaptureController,
        theme_override: Option<CaptureThemeProfile>,
        snapshot_scene: Option<UiSnapshotScene>,
        open_requests: OpenRequestReceiver,
        native_menu_commands: NativeMenuReceiver,
        mut settings: AppSettings,
        window_host: EditorWindowHost,
    ) -> Self {
        let invalid_initial_path = initial_path
            .as_ref()
            .filter(|path| !path.is_file() && !path.is_dir())
            .cloned();
        let working_directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let initial_workspace =
            resolve_initial_workspace(&settings, initial_path.as_deref(), &working_directory);
        let workspace_root = initial_workspace.root;
        let initial_document = initial_workspace.document;
        if invalid_initial_path.is_none() {
            settings.remember_workspace(&workspace_root);
        }
        let applied_theme_request =
            active_theme_request(&settings, context.system_theme(), theme_override.as_ref());
        let (active_theme, initial_theme_error) =
            load_active_theme_or_fallback(&applied_theme_request);
        theme::set_imported_palette(Some((active_theme.dark_mode, active_theme.palette)));
        let typst_tool = resolve_tool(ToolKind::Typst, &settings.typst);
        let tinymist_tool = resolve_tool(ToolKind::Tinymist, &settings.tinymist);
        context.options_mut(|options| {
            options.zoom_with_keyboard = false;
            options.sync_window_theme = true;
            options.fallback_theme = egui::Theme::Dark;
        });
        theme::configure_editor_fonts(context);
        theme::configure_styles(context);
        context.set_theme(active_theme_preference(
            settings.interface_theme,
            theme_override.is_some(),
            active_theme.dark_mode,
        ));
        let active_interface_theme = if active_theme.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        let preview_dark =
            settings.document_theme.resolve(active_interface_theme) == egui::Theme::Dark;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let (web_link_sender, web_link_receiver) = mpsc::channel();

        let mut generic_highlighter = GenericSyntaxHighlighter::default();
        generic_highlighter.set_custom_theme(Some(active_theme.syntect_theme.clone()));
        let mut highlighter = SyntaxHighlighter::default();
        highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::syntax_palette(active_theme.dark_mode),
            Some(&active_theme.syntect_theme),
            settings.typst_overrides.for_dark(active_theme.dark_mode),
        ));
        let mut app = Self {
            source: DEFAULT_SOURCE.to_owned(),
            saved_source: DEFAULT_SOURCE.to_owned(),
            path: None,
            document_epoch: 0,
            revision: 0,
            disk_fingerprint: None,
            document_kind: DocumentKind::Typst,
            highlighter,
            generic_highlighter,
            compiler: Compiler::new(context.clone()),
            asset_loader: AssetLoader::new(context.clone()),
            asset_token: 0,
            pending_asset_page: None,
            compile_deadline: Some(Instant::now()),
            status: PreviewStatus::Waiting,
            raw_diagnostics: String::new(),
            diagnostics: Vec::new(),
            tinymist_diagnostics: Vec::new(),
            compiled_revision: None,
            pdf: None,
            pages: Vec::new(),
            visible_page: 0,
            zoom: 1.0,
            fit_width: true,
            requested_zoom: None,
            requested_page: None,
            preview_dark,
            preview_was_visible: false,
            view_mode: ViewMode::Split,
            filesystem_visible: true,
            problems_visible: false,
            settings_visible: false,
            typst_overrides_visible: false,
            typst_overrides_dark: active_theme.dark_mode,
            settings: settings.clone(),
            pending_settings: None,
            imported_theme: Some(active_theme),
            applied_theme_request,
            applied_typst_overrides: settings.typst_overrides.clone(),
            theme_override,
            applied_document_theme: settings.document_theme,
            applied_preview_preference: settings.preview_preference,
            applied_typst_preference: settings.typst.clone(),
            applied_tinymist_preference: settings.tinymist.clone(),
            tool_refresh_requested: false,
            typst_tool,
            tinymist_tool,
            workspace_root,
            workspace_chooser_visible: false,
            workspace: None,
            workspace_error: None,
            workspace_scan: None,
            workspace_scan_id: 0,
            next_workspace_refresh: Instant::now(),
            project_index: ProjectIndex::default(),
            project_index_deadline: Some(Instant::now()),
            project_index_result: None,
            project_index_request_id: 0,
            captures,
            snapshot_scene,
            window_host,
            pending_window_requests: VecDeque::new(),
            open_requests,
            native_menu_commands,
            queued_native_menu_commands: NativeMenuCommandQueue::default(),
            queued_open_requests: VecDeque::new(),
            find_visible: false,
            replace_visible: false,
            find_query: String::new(),
            replacement: String::new(),
            search: SearchState::default(),
            focus_find: false,
            pending_editor_selection: None,
            editor_attention: None,
            editor_hover: None,
            next_editor_hover_token: 1,
            reset_editor_history: true,
            editor_undo: Vec::new(),
            editor_redo: Vec::new(),
            autosave_deadline: None,
            diagnostic_tooltip: None,
            app_popup: None,
            app_popup_had_focus: false,
            pending_app_popup_action: None,
            app_modal: None,
            app_modal_had_focus: false,
            app_modal_suspended: false,
            pending_document_action: None,
            post_save_action: None,
            rename_dialog: None,
            rename_overlay_had_focus: false,
            rename_overlay_suspended: false,
            pending_export: None,
            pending_export_dialog: None,
            pending_tool_picker: None,
            pending_document_dialog: None,
            notice: None,
            status_log: VecDeque::new(),
            recorded_status: None,
            last_title: String::new(),
            allow_close: false,
            tinymist: TinymistSidecar::new(context.clone()),
            tinymist_generation: None,
            tinymist_uri: None,
            tinymist_preview_uri: None,
            tinymist_current_open: false,
            tinymist_unsaved_document: None,
            tinymist_url: None,
            tinymist_preview_enabled: false,
            tinymist_lsp_ready: false,
            tinymist_state: ServiceState::Starting("Launching Tinymist LSP".to_owned()),
            webview_state: ServiceState::Starting(
                "Waiting for Tinymist's preview server".to_owned(),
            ),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            native_window_parent: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview_url: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            web_link_sender,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            web_link_receiver,
        };

        if let Some(error) = initial_theme_error {
            app.notice = Some(Notice {
                message: error,
                kind: NoticeKind::Error,
            });
        }
        if let Some(path) = invalid_initial_path {
            app.notice = Some(Notice {
                message: format!("{} does not exist", canonical_or_absolute(&path).display()),
                kind: NoticeKind::Error,
            });
        }

        if let Some(path) = initial_document {
            if !app.load_path(path) {
                app.reset_document_services();
                app.schedule_compile_now();
            }
        } else {
            app.reset_document_services();
        }
        app
    }

    pub(crate) fn new_secondary(
        context: &egui::Context,
        request: EditorWindowRequest,
        settings: AppSettings,
        captures: CaptureController,
    ) -> Self {
        let (_, open_requests) = crate::open_requests::channel();
        let (_, native_menu_commands) = crate::native_menu::channel();
        let (initial_path, untitled_workspace) = match &request {
            EditorWindowRequest::New { workspace_root } => {
                (Some(workspace_root.clone()), Some(workspace_root.clone()))
            }
            EditorWindowRequest::Open(path) => (Some(path.clone()), None),
        };
        let mut app = Self::new_session(
            context,
            initial_path,
            captures,
            None,
            None,
            open_requests,
            native_menu_commands,
            settings,
            EditorWindowHost::Secondary,
        );
        if let Some(workspace_root) = untitled_workspace {
            app.workspace_root = canonical_or_absolute(&workspace_root);
            app.remember_workspace(&app.workspace_root.clone());
            app.reset_untitled_document();
            app.refresh_workspace();
        }
        app
    }

    pub(crate) fn ui_in_window(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        <Self as eframe::App>::ui(self, ui, frame);
    }

    /// Assign a process-level native menu command to this document session.
    /// It is deliberately executed later from `ui_in_window`, where `context`
    /// belongs to this session's viewport and therefore owns its TextEditState.
    pub(crate) fn enqueue_native_menu_command(&mut self, command: NativeMenuCommand) {
        self.queued_native_menu_commands.push(command);
    }

    pub(crate) fn settings_snapshot(&self) -> AppSettings {
        self.pending_settings
            .as_ref()
            .unwrap_or(&self.settings)
            .clone()
    }

    pub(crate) fn take_window_request(&mut self) -> Option<EditorWindowRequest> {
        self.pending_window_requests.pop_front()
    }

    pub(crate) fn can_reuse_for_external_open(&self) -> bool {
        self.path.is_none() && !self.is_dirty() && !self.document_flow_busy()
    }

    pub(crate) fn open_external_path(&mut self, path: PathBuf) {
        self.queued_open_requests.push_back(path);
    }

    pub(crate) fn window_title(&self) -> String {
        self.title()
    }

    pub(crate) fn is_dirty_for_close(&self) -> bool {
        self.is_dirty()
    }

    pub(crate) fn close_accepted(&self) -> bool {
        self.allow_close
    }

    pub(crate) fn show_window_notice(&mut self, message: String) {
        self.notice = Some(Notice {
            message,
            kind: NoticeKind::Info,
        });
    }

    fn is_dirty(&self) -> bool {
        self.document_kind.is_editable() && self.source != self.saved_source
    }

    fn document_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.typ".to_owned())
    }

    fn title(&self) -> String {
        let dirty = if self.is_dirty() { "*" } else { "" };
        format!("{}{dirty} — tiptoptyp", self.document_name())
    }

    fn update_title(&mut self, context: &egui::Context) {
        let title = self.title();
        if title != self.last_title {
            context.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn current_directory(&self) -> Option<PathBuf> {
        self.path
            .as_ref()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf)
            .or_else(|| {
                self.workspace_root
                    .is_dir()
                    .then(|| self.workspace_root.clone())
            })
            .or_else(|| std::env::current_dir().ok())
    }

    fn project_root(&self) -> PathBuf {
        self.workspace_root.clone()
    }

    fn designated_preview_path(&self) -> Option<PathBuf> {
        let root = canonical_or_absolute(&self.workspace_root);
        let key = root.to_str()?;
        let value = self.settings.preview_files.get(key)?;
        let path = canonical_or_absolute(Path::new(value));
        (path.starts_with(&root)
            && path.is_file()
            && path.extension().is_some_and(|extension| extension == "typ"))
        .then_some(path)
    }

    fn preview_document_path(&self) -> PathBuf {
        self.designated_preview_path()
            .or_else(|| {
                (self.document_kind.is_typst())
                    .then(|| self.path.clone())
                    .flatten()
            })
            .unwrap_or_else(|| {
                self.project_root()
                    .join(".tiptoptyp")
                    .join("documents")
                    .join("untitled.typ")
            })
    }

    fn current_is_preview_document(&self) -> bool {
        match &self.path {
            Some(path) => same_path(path, &self.preview_document_path()),
            None => self.designated_preview_path().is_none() && self.document_kind.is_typst(),
        }
    }

    fn preview_document_source(&self) -> Result<String, String> {
        let path = self.preview_document_path();
        if self.current_is_preview_document() {
            return Ok(self.source.clone());
        }
        fs::read_to_string(&path)
            .map_err(|error| format!("Could not read preview entry {}: {error}", path.display()))
    }

    fn schedule_project_index(&mut self) {
        self.project_index_deadline = Some(Instant::now() + PROJECT_INDEX_DEBOUNCE);
    }

    fn rebuild_project_index(&mut self, context: &egui::Context) {
        self.project_index_deadline = None;
        let main = self.preview_document_path();
        let root = self.project_root();
        let mut overrides = BTreeMap::new();
        if self.document_kind.is_typst() {
            let path = self.path.clone().unwrap_or_else(|| main.clone());
            overrides.insert(canonical_or_absolute(&path), self.source.clone());
        }
        self.project_index_request_id = self.project_index_request_id.wrapping_add(1);
        let request_id = self.project_index_request_id;
        let (sender, receiver) = mpsc::channel();
        self.project_index_result = Some(receiver);
        let repaint = context.clone();
        let _ = thread::Builder::new()
            .name("tiptoptyp-project-index".to_owned())
            .spawn(move || {
                let index = analyze_project(&root, &main, &overrides);
                let _ = sender.send((request_id, index));
                repaint.request_repaint();
            });
    }

    fn tick_project_index(&mut self, context: &egui::Context) {
        if let Some(result) = self
            .project_index_result
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            self.project_index_result = None;
            if result.0 == self.project_index_request_id && self.project_index_deadline.is_none() {
                self.project_index = result.1;
            }
        }
        if self.project_index_result.is_some() {
            context.request_repaint_after(Duration::from_millis(50));
        }
        let Some(deadline) = self.project_index_deadline else {
            return;
        };
        let now = Instant::now();
        if deadline <= now {
            self.rebuild_project_index(context);
        } else {
            context.request_repaint_after(deadline - now);
        }
    }

    fn use_file_for_preview(&mut self, path: PathBuf, context: &egui::Context) {
        let root = canonical_or_absolute(&self.workspace_root);
        let path = canonical_or_absolute(&path);
        if !path.starts_with(&root)
            || !path.is_file()
            || path.extension().is_none_or(|extension| extension != "typ")
        {
            self.notice = Some(Notice {
                message: "Only a Typst file inside the current project can be used for preview"
                    .to_owned(),
                kind: NoticeKind::Error,
            });
            return;
        }
        let (Some(key), Some(value)) = (root.to_str(), path.to_str()) else {
            return;
        };
        self.settings
            .preview_files
            .insert(key.to_owned(), value.to_owned());
        if let Some(settings) = &mut self.pending_settings {
            settings
                .preview_files
                .insert(key.to_owned(), value.to_owned());
        }
        self.rebuild_project_index(context);
        self.restart_tinymist();
        self.schedule_compile_now();
        self.notice = Some(Notice {
            message: format!("Using {} for preview", path.display()),
            kind: NoticeKind::Success,
        });
    }

    fn remember_open_document(&mut self, path: &Path) {
        let Some(parent) = path.parent() else {
            return;
        };
        let root = if path.starts_with(&self.workspace_root) {
            self.workspace_root.clone()
        } else {
            canonical_or_absolute(&discover_project_root(parent))
        };
        let path = canonical_or_absolute(path);
        let (Some(key), Some(value)) = (root.to_str(), path.to_str()) else {
            return;
        };
        let key = key.to_owned();
        let value = value.to_owned();
        self.settings
            .last_opened_files
            .insert(key.clone(), value.clone());
        if let Some(pending) = &mut self.pending_settings {
            pending.last_opened_files.insert(key.clone(), value.clone());
        }
        trim_file_history(&mut self.settings.last_opened_files, &key);
        if let Some(pending) = &mut self.pending_settings {
            trim_file_history(&mut pending.last_opened_files, &key);
        }
    }

    fn mark_edited(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.notice = None;
        self.editor_hover = None;
        if self.document_kind.is_typst() {
            // Tinymist sees unsaved edits in every open source file. The CLI
            // fallback can only see an imported subfile after it is saved, so
            // avoid rebuilding the designated main entry with stale disk data.
            self.compile_deadline = self
                .current_is_preview_document()
                .then(|| Instant::now() + COMPILE_DEBOUNCE);
            if self.compile_deadline.is_some() {
                self.status = PreviewStatus::Waiting;
            }
            self.sync_tinymist_change();
            self.schedule_project_index();
        } else {
            self.compile_deadline = None;
        }
        self.schedule_autosave_if_needed();
    }

    fn schedule_autosave_if_needed(&mut self) {
        self.autosave_deadline = (self.settings.auto_save
            && self.path.is_some()
            && self.source != self.saved_source)
            .then(|| {
                Instant::now() + Duration::from_millis(self.settings.auto_save_delay_ms.max(100))
            });
    }

    fn tick_autosave(&mut self, context: &egui::Context) {
        if self.pending_document_dialog.is_some() || self.app_modal.is_some() {
            return;
        }
        let Some(deadline) = self.autosave_deadline else {
            return;
        };
        if !self.settings.auto_save || !self.is_dirty() {
            self.autosave_deadline = None;
            return;
        }
        let now = Instant::now();
        if deadline > now {
            context.request_repaint_after(deadline - now);
            return;
        }
        self.autosave_deadline = None;
        let Some(path) = self.path.clone() else {
            return;
        };

        if self.save_to_with_intent(path, SaveIntent::Auto) {
            self.notice = Some(Notice {
                message: "Saved automatically".to_owned(),
                kind: NoticeKind::Success,
            });
        }
    }

    fn request_compile(&mut self) {
        self.compile_deadline = None;
        if !self.document_kind.is_typst() || !self.preview_processing_enabled() {
            return;
        }
        let preview_path = self.preview_document_path();
        let source = match self.preview_document_source() {
            Ok(source) => source,
            Err(error) => {
                self.set_compile_error(error);
                return;
            }
        };
        let source_dir = preview_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.project_root());
        let display_name = preview_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.typ".to_owned());
        let request = CompileRequest {
            revision: self.revision,
            source,
            project_root: self.project_root(),
            source_dir,
            display_name,
            typst_executable: self.typst_tool.program.clone(),
        };

        match self.compiler.request(request) {
            Ok(()) => self.status = PreviewStatus::Compiling,
            Err(error) => self.set_compile_error(error),
        }
    }

    fn tick_compile(&mut self, context: &egui::Context) {
        if !self.preview_processing_enabled() {
            self.compile_deadline = None;
            return;
        }
        let Some(deadline) = self.compile_deadline else {
            return;
        };
        let now = Instant::now();
        if deadline <= now {
            self.request_compile();
        } else {
            context.request_repaint_after(deadline - now);
        }
    }

    fn receive_compile_results(&mut self, context: &egui::Context) {
        while let Some(result) = self.compiler.try_recv() {
            if !self.document_kind.is_typst()
                || !self.preview_processing_enabled()
                || result.revision != self.revision
            {
                continue;
            }

            let Some(output) = result.output else {
                // Keep both the last successful pages and previous diagnostics
                // in place until a terminal result arrives. Neither transient
                // state changes preview geometry.
                self.status = PreviewStatus::Compiling;
                continue;
            };

            match output {
                Ok(document) => {
                    self.pages = document
                        .pages
                        .into_iter()
                        .enumerate()
                        .map(|(index, page)| {
                            make_preview_texture(
                                context,
                                result.revision,
                                index,
                                page,
                                self.preview_dark,
                            )
                        })
                        .collect();
                    self.visible_page = self.visible_page.min(self.pages.len().saturating_sub(1));
                    self.pdf = Some(document.pdf);
                    self.set_diagnostics(document.diagnostics);
                    self.compiled_revision = Some(result.revision);
                    self.status = PreviewStatus::Ready(result.elapsed);
                    self.complete_pending_export();
                }
                Err(error) => self.set_compile_error(error),
            }
        }
    }

    fn receive_asset_results(&mut self, context: &egui::Context) {
        while let Some(result) = self.asset_loader.try_recv() {
            if result.token != self.asset_token || !self.document_kind.preview_only() {
                continue;
            }
            match result.output {
                Ok(LoadedAsset::Image(page)) => {
                    let image_size = page.size;
                    let mut texture = make_preview_texture(context, self.revision, 0, page, false);
                    // Native PDF pages are 144-DPI rasters interpreted in
                    // 72-point coordinates. Doubling the image's logical
                    // raster size cancels that conversion and makes 100% mean
                    // one source image pixel per UI point.
                    texture.size = [
                        image_size[0].saturating_mul(2),
                        image_size[1].saturating_mul(2),
                    ];
                    self.pages = vec![texture];
                    self.pdf = None;
                    self.compiled_revision = Some(self.revision);
                    self.status = PreviewStatus::Ready(Duration::ZERO);
                }
                Ok(LoadedAsset::Pdf { bytes, pages }) => {
                    self.pages = pages
                        .into_iter()
                        .enumerate()
                        .map(|(index, page)| {
                            make_preview_texture(
                                context,
                                self.revision,
                                index,
                                page,
                                self.preview_dark,
                            )
                        })
                        .collect();
                    self.pdf = Some(bytes);
                    self.compiled_revision = Some(self.revision);
                    self.status = PreviewStatus::Ready(Duration::ZERO);
                    if let Some(page) = self.pending_asset_page.take() {
                        self.requested_page = Some(page.min(self.pages.len().saturating_sub(1)));
                    }
                    self.complete_pending_export();
                }
                Err(error) => {
                    self.pending_export = None;
                    self.status = PreviewStatus::Error;
                    self.notice = Some(Notice {
                        message: error,
                        kind: NoticeKind::Error,
                    });
                }
            }
        }
    }

    fn set_compile_error(&mut self, error: String) {
        self.compiled_revision = None;
        self.status = PreviewStatus::Error;
        self.set_diagnostics(error);
    }

    fn set_diagnostics(&mut self, raw: String) {
        let display_name = self
            .preview_document_path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.document_name());
        self.diagnostics = parse_typst_short_output(&raw, Some(Path::new(&display_name)));
        self.raw_diagnostics = raw;
    }

    fn schedule_compile_now(&mut self) {
        if !self.document_kind.is_typst() || !self.preview_processing_enabled() {
            self.compile_deadline = None;
            return;
        }
        self.compile_deadline = Some(Instant::now());
        self.status = PreviewStatus::Waiting;
    }

    fn clear_preview_for_document(&mut self) {
        self.asset_token = self.asset_token.wrapping_add(1);
        self.asset_loader.cancel_before(self.asset_token);
        self.compiled_revision = None;
        self.pdf = None;
        self.pages.clear();
        self.visible_page = 0;
        self.raw_diagnostics.clear();
        self.diagnostics.clear();
        self.tinymist_diagnostics.clear();
        self.pending_asset_page = None;
        self.editor_hover = None;
        if self.pending_export.take().is_some() {
            self.notice = Some(Notice {
                message: "Queued PDF export canceled because another document is now active"
                    .to_owned(),
                kind: NoticeKind::Info,
            });
        }
    }

    fn handle_shortcuts(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        let save = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::S);
        let save_as = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::S);
        let open = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::O);
        let open_in_new_window =
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, egui::Key::O);
        let open_folder =
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::O);
        let new = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::N);
        let new_window = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::N);
        let refresh = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::R);
        let export = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::E);
        let find = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::F);
        let settings = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Comma);
        let undo = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Z);
        let redo = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Z);
        let format = format_shortcut();
        let replace_macos =
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, egui::Key::F);
        let replace_windows = KeyboardShortcut::new(Modifiers::CTRL, egui::Key::H);

        let view_action = context.input_mut(|input| {
            if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Num1)) {
                Some(ViewCommand::Explorer)
            } else if input
                .consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Num2))
            {
                Some(ViewCommand::Code)
            } else if input
                .consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Num3))
            {
                Some(ViewCommand::Split)
            } else if input
                .consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Num4))
            {
                Some(ViewCommand::Preview)
            } else if input
                .consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Num5))
            {
                Some(ViewCommand::Problems)
            } else {
                None
            }
        });
        if let Some(action) = view_action {
            match action {
                ViewCommand::Explorer => self.filesystem_visible = !self.filesystem_visible,
                ViewCommand::Problems => self.problems_visible = !self.problems_visible,
                ViewCommand::Code => self.view_mode = ViewMode::Code,
                ViewCommand::Split => self.view_mode = ViewMode::Split,
                ViewCommand::Preview => self.view_mode = ViewMode::Preview,
            }
        }

        let file_action = context.input_mut(|input| {
            if input.consume_shortcut(&save_as) {
                Some(FileMenuAction::SaveAs)
            } else if input.consume_shortcut(&save) {
                Some(FileMenuAction::Save)
            } else if input.consume_shortcut(&open) {
                Some(FileMenuAction::Open)
            } else if input.consume_shortcut(&open_in_new_window) {
                Some(FileMenuAction::OpenInNewWindow)
            } else if input.consume_shortcut(&open_folder) {
                Some(FileMenuAction::ChangeWorkspaceRoot)
            } else if input.consume_shortcut(&new_window) {
                Some(FileMenuAction::NewWindow)
            } else if input.consume_shortcut(&new) {
                Some(FileMenuAction::New)
            } else if input.consume_shortcut(&export) {
                Some(FileMenuAction::ExportPdf)
            } else {
                None
            }
        });
        if let Some(action) = file_action {
            self.execute_file_menu_action(action, frame);
        }
        if context.input_mut(|input| input.consume_shortcut(&refresh)) {
            self.request_compile();
            self.refresh_workspace();
        }
        if context.input_mut(|input| input.consume_shortcut(&find)) {
            self.open_find(false);
        }
        if context.input_mut(|input| input.consume_shortcut(&settings)) {
            self.settings_visible = !self.settings_visible;
        }
        let editor_id = source_editor_id(context);
        let source_focused = context.memory(|memory| memory.focused()) == Some(editor_id);
        if source_focused || !context.egui_wants_keyboard_input() {
            if context.input_mut(|input| input.consume_shortcut(&redo)) {
                self.undo_editor(context, true);
            } else if context.input_mut(|input| input.consume_shortcut(&undo)) {
                self.undo_editor(context, false);
            }
        }
        if context.input_mut(|input| input.consume_shortcut(&format)) {
            self.request_format_document();
        }
        if context.input_mut(|input| input.consume_shortcut(&replace_macos))
            || context.input_mut(|input| input.consume_shortcut(&replace_windows))
        {
            self.open_find(true);
        }
        if self.find_visible
            && context.input_mut(|input| input.consume_key(Modifiers::NONE, egui::Key::Escape))
        {
            self.find_visible = false;
            self.replace_visible = false;
            self.search.clear();
        }

        if self.view_mode.shows_preview() && !self.interactive_preview_active() {
            let zoom_in = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Plus);
            let zoom_in_secondary = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Equals);
            let zoom_out = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Minus);
            let zoom_reset = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Num0);
            if context.input_mut(|input| input.consume_shortcut(&zoom_in))
                || context.input_mut(|input| input.consume_shortcut(&zoom_in_secondary))
            {
                self.requested_zoom = Some(
                    (self.zoom * METRICS.preview.zoom_step)
                        .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                );
                self.fit_width = false;
            }
            if context.input_mut(|input| input.consume_shortcut(&zoom_out)) {
                self.requested_zoom = Some(
                    (self.zoom / METRICS.preview.zoom_step)
                        .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                );
                self.fit_width = false;
            }
            if context.input_mut(|input| input.consume_shortcut(&zoom_reset)) {
                self.requested_zoom = Some(1.0);
                self.fit_width = false;
            }
        }
    }

    fn execute_file_menu_action(&mut self, action: FileMenuAction, frame: &eframe::Frame) {
        if self.document_flow_busy() {
            self.notice = Some(Notice {
                message: "Finish the current file operation before starting another".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        match action {
            FileMenuAction::New => self.new_document(),
            FileMenuAction::NewWindow => {
                self.pending_window_requests
                    .push_back(EditorWindowRequest::New {
                        workspace_root: self.workspace_root.clone(),
                    });
            }
            FileMenuAction::Open => self.open_dialog(),
            FileMenuAction::OpenInNewWindow => self.open_in_new_window_dialog(frame),
            FileMenuAction::ChangeWorkspaceRoot => self.open_workspace_chooser(),
            FileMenuAction::Save => {
                self.save_document(frame);
            }
            FileMenuAction::SaveAs => {
                self.save_as(frame);
            }
            FileMenuAction::ExportPdf => {
                self.export_pdf(frame);
            }
        }
    }

    fn execute_editor_menu_action(&mut self, action: EditorMenuAction, context: &egui::Context) {
        match action {
            EditorMenuAction::Undo => self.undo_editor(context, false),
            EditorMenuAction::Redo => self.undo_editor(context, true),
            EditorMenuAction::Cut => self.cut_editor_selection(context),
            EditorMenuAction::Copy => self.copy_editor_selection(context),
            EditorMenuAction::Paste => self.request_editor_paste(context),
            EditorMenuAction::SelectAll => self.select_all_editor(context),
            EditorMenuAction::Format => self.request_format_document(),
        }
    }

    fn receive_native_menu_commands(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        let commands = self.native_menu_commands.pending().collect::<Vec<_>>();
        self.queued_native_menu_commands.extend(commands);
        while let Some(command) = self.queued_native_menu_commands.pop() {
            self.dispatch_native_menu_command(command, context, frame);
        }
    }

    fn dispatch_native_menu_command(
        &mut self,
        command: NativeMenuCommand,
        context: &egui::Context,
        frame: &eframe::Frame,
    ) {
        match command {
            NativeMenuCommand::Application(ApplicationCommand::Settings) => {
                self.settings_visible = true;
            }
            NativeMenuCommand::File(command) => {
                let action = match command {
                    FileCommand::New => FileMenuAction::New,
                    FileCommand::NewWindow => FileMenuAction::NewWindow,
                    FileCommand::Open => FileMenuAction::Open,
                    FileCommand::OpenInNewWindow => FileMenuAction::OpenInNewWindow,
                    FileCommand::ChangeWorkspaceRoot => FileMenuAction::ChangeWorkspaceRoot,
                    FileCommand::Save => FileMenuAction::Save,
                    FileCommand::SaveAs => FileMenuAction::SaveAs,
                    FileCommand::ExportPdf => FileMenuAction::ExportPdf,
                };
                self.execute_file_menu_action(action, frame);
            }
            NativeMenuCommand::Edit(command) => match command {
                EditCommand::Find => self.open_find(false),
                EditCommand::FindReplace => self.open_find(true),
                EditCommand::Undo => {
                    self.execute_editor_menu_action(EditorMenuAction::Undo, context)
                }
                EditCommand::Redo => {
                    self.execute_editor_menu_action(EditorMenuAction::Redo, context)
                }
                EditCommand::Cut => self.execute_editor_menu_action(EditorMenuAction::Cut, context),
                EditCommand::Copy => {
                    self.execute_editor_menu_action(EditorMenuAction::Copy, context)
                }
                EditCommand::Paste => {
                    self.execute_editor_menu_action(EditorMenuAction::Paste, context)
                }
                EditCommand::SelectAll => {
                    self.execute_editor_menu_action(EditorMenuAction::SelectAll, context)
                }
                EditCommand::Format => {
                    self.execute_editor_menu_action(EditorMenuAction::Format, context)
                }
            },
            NativeMenuCommand::View(command) => match command {
                ViewCommand::Problems => self.problems_visible = !self.problems_visible,
                ViewCommand::Explorer => self.filesystem_visible = !self.filesystem_visible,
                ViewCommand::Code => self.view_mode = ViewMode::Code,
                ViewCommand::Split => self.view_mode = ViewMode::Split,
                ViewCommand::Preview => self.view_mode = ViewMode::Preview,
            },
        }
    }

    fn document_flow_busy(&self) -> bool {
        self.app_modal.is_some()
            || self.rename_dialog.is_some()
            || self.pending_document_action.is_some()
            || self.post_save_action.is_some()
            || self.pending_document_dialog.is_some()
            || self.pending_export_dialog.is_some()
            || self.pending_tool_picker.is_some()
    }

    fn open_find(&mut self, replace: bool) {
        self.find_visible = true;
        self.replace_visible |= replace;
        self.focus_find = true;
        if !self.view_mode.shows_code() {
            self.view_mode = ViewMode::Split;
        }
    }

    fn queue_settings(&mut self, settings: AppSettings, context: &egui::Context) {
        if settings == self.settings {
            self.pending_settings = None;
        } else {
            self.pending_settings = Some(settings);
            context.request_repaint();
        }
    }

    fn apply_snapshot_scene(&mut self) {
        let Some(scene) = self.snapshot_scene else {
            return;
        };
        if matches!(
            scene,
            UiSnapshotScene::Main | UiSnapshotScene::ProblemsPanel | UiSnapshotScene::FindReplace
        ) && self.pages.is_empty()
        {
            self.captures.defer_target("main");
        }
        let toolbar_anchor = Pos2::new(theme::SPACE.content, METRICS.chrome.toolbar_height);
        match scene {
            UiSnapshotScene::Main => {
                self.status = PreviewStatus::Ready(Duration::ZERO);
                self.notice = None;
                self.raw_diagnostics.clear();
                self.diagnostics.clear();
                self.tinymist_diagnostics.clear();
                self.problems_visible = false;
                self.find_visible = false;
                self.replace_visible = false;
            }
            UiSnapshotScene::FileMenu => {
                self.app_popup = Some(AppPopup::File {
                    anchor: toolbar_anchor + egui::vec2(150.0, 0.0),
                });
            }
            UiSnapshotScene::EditMenu => {
                self.app_popup = Some(AppPopup::Edit {
                    anchor: toolbar_anchor + egui::vec2(195.0, 0.0),
                });
            }
            UiSnapshotScene::SettingsWindow
            | UiSnapshotScene::SettingsThemePicker
            | UiSnapshotScene::SettingsDarkThemePicker
            | UiSnapshotScene::SettingsTooltip => self.settings_visible = true,
            UiSnapshotScene::TypstOverridesWindow => {
                self.settings_visible = false;
                self.typst_overrides_visible = true;
                self.typst_overrides_dark = self
                    .imported_theme
                    .as_ref()
                    .is_none_or(|theme| theme.dark_mode);
            }
            // These scenes are injected after the editor paints, because the
            // editor intentionally replaces hover overlays every frame.
            UiSnapshotScene::DiagnosticTooltip | UiSnapshotScene::FunctionTooltip => {}
            UiSnapshotScene::SaveDialog => {
                if self.app_modal.is_none() {
                    self.app_modal = Some(AppModal::Unsaved {
                        message: format!(
                            "Save changes to {} before opening another file?",
                            self.document_name()
                        ),
                        pending: PendingDocumentAction {
                            action: DeferredDocumentAction::CloseWindow,
                            document_epoch: self.document_epoch,
                            revision: self.revision,
                            allow_discard: true,
                            description: "closing the document".to_owned(),
                        },
                    });
                }
            }
            UiSnapshotScene::AlertDialog => {
                if self.app_modal.is_none() {
                    self.app_modal = Some(AppModal::Alert {
                        title: "error".to_owned(),
                        message:
                            "The document could not be saved. Check the destination and try again."
                                .to_owned(),
                        kind: NoticeKind::Error,
                    });
                }
            }
            UiSnapshotScene::OverwriteDialog => {
                if self.app_modal.is_none() {
                    self.app_modal = Some(AppModal::Overwrite {
                        message: "This file changed on disk after it was opened. Overwrite it with the editor contents?"
                            .to_owned(),
                        path: self
                            .path
                            .clone()
                            .unwrap_or_else(|| self.workspace_root.join("document.typ")),
                        document_epoch: self.document_epoch,
                        revision: self.revision,
                        expected_disk_fingerprint: Some(1),
                        observed_disk_fingerprint: Some(2),
                    });
                }
            }
            UiSnapshotScene::EditorContextMenu => {
                self.app_popup = Some(AppPopup::Editor {
                    anchor: Pos2::new(430.0, 250.0),
                });
            }
            UiSnapshotScene::ExplorerContextMenu => {
                let path = self
                    .path
                    .clone()
                    .unwrap_or_else(|| self.workspace_root.join("document.typ"));
                self.app_popup = Some(AppPopup::Workspace {
                    anchor: Pos2::new(180.0, 180.0),
                    path,
                    is_file: true,
                });
            }
            UiSnapshotScene::StatusLog => {
                self.status = PreviewStatus::Ready(Duration::from_millis(18));
                self.recorded_status = Some(self.status);
                self.status_log = VecDeque::from([
                    StatusLogEntry {
                        detail: "PDF ready in 18 ms".to_owned(),
                        kind: NoticeKind::Success,
                    },
                    StatusLogEntry {
                        detail: "Compiling document".to_owned(),
                        kind: NoticeKind::Info,
                    },
                    StatusLogEntry {
                        detail: "PDF ready in 24 ms".to_owned(),
                        kind: NoticeKind::Success,
                    },
                    StatusLogEntry {
                        detail: "Waiting for changes".to_owned(),
                        kind: NoticeKind::Info,
                    },
                ]);
                self.app_popup = Some(AppPopup::StatusLog {
                    anchor: Pos2::new(theme::SPACE.content, 500.0),
                });
            }
            UiSnapshotScene::RenameDialog => {
                if self.rename_dialog.is_none() {
                    let path = self
                        .path
                        .clone()
                        .unwrap_or_else(|| self.workspace_root.join("document.typ"));
                    let name = path.file_name().map_or_else(
                        || "document.typ".to_owned(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                    self.rename_dialog = Some(RenameDialog {
                        path,
                        name,
                        focus: false,
                    });
                }
            }
            UiSnapshotScene::WorkspaceChooser => self.workspace_chooser_visible = true,
            UiSnapshotScene::ProblemsPanel => {
                self.problems_visible = true;
                self.status = PreviewStatus::Error;
                self.diagnostics = vec![
                    Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        source: DiagnosticSource::Main,
                        location: Some(DiagnosticLocation {
                            line: 9,
                            column: 29,
                        }),
                        message: "the character `#` is not valid in code".to_owned(),
                        details: vec![
                            "Hint: you are already in code mode".to_owned(),
                            "Hint: try removing the `#`".to_owned(),
                        ],
                    },
                    Diagnostic {
                        severity: DiagnosticSeverity::Warning,
                        source: DiagnosticSource::Main,
                        location: Some(DiagnosticLocation {
                            line: 14,
                            column: 1,
                        }),
                        message: "unknown font family; using a fallback".to_owned(),
                        details: Vec::new(),
                    },
                ];
                self.tinymist_diagnostics.clear();
                self.raw_diagnostics.clear();
            }
            UiSnapshotScene::FindReplace => {
                self.status = PreviewStatus::Ready(Duration::ZERO);
                self.notice = None;
                self.find_visible = true;
                self.replace_visible = true;
                self.find_query = "Typst".to_owned();
                self.replacement = "tiptoptyp".to_owned();
            }
            UiSnapshotScene::PreviewCompiling => {
                self.status = PreviewStatus::Compiling;
                self.pdf = None;
                self.pages.clear();
                self.compiled_revision = None;
            }
        }
    }

    fn sync_runtime_settings(&mut self, context: &egui::Context) {
        let theme_request = active_theme_request(
            &self.settings,
            context.system_theme(),
            self.theme_override.as_ref(),
        );
        if self.applied_theme_request != theme_request {
            self.applied_theme_request = theme_request;
            self.reload_active_theme(context);
        }
        if self.applied_typst_overrides != self.settings.typst_overrides {
            self.applied_typst_overrides = self.settings.typst_overrides.clone();
            if let Some(active) = &self.imported_theme {
                let styles = ResolvedTypstStyles::resolve(
                    theme::syntax_palette(active.dark_mode),
                    Some(&active.syntect_theme),
                    self.settings.typst_overrides.for_dark(active.dark_mode),
                );
                self.highlighter.set_styles(styles);
            }
        }
        let active_interface_theme =
            self.imported_theme
                .as_ref()
                .map_or(context.theme(), |theme| {
                    if theme.dark_mode {
                        egui::Theme::Dark
                    } else {
                        egui::Theme::Light
                    }
                });
        let next_preview_dark =
            self.settings.document_theme.resolve(active_interface_theme) == egui::Theme::Dark;
        let preview_appearance_changed = next_preview_dark != self.preview_dark;
        if preview_appearance_changed {
            self.preview_dark = next_preview_dark;
            self.rebuild_preview_textures(context);
        }
        let document_theme_mode_changed =
            self.applied_document_theme != self.settings.document_theme;
        if document_theme_mode_changed {
            self.applied_document_theme = self.settings.document_theme;
        }

        let preview_preference_changed =
            self.applied_preview_preference != self.settings.preview_preference;
        if preview_preference_changed {
            self.applied_preview_preference = self.settings.preview_preference;
        }

        let refresh_tools = std::mem::take(&mut self.tool_refresh_requested);
        let typst_preference_changed = self.applied_typst_preference != self.settings.typst;
        if refresh_tools || typst_preference_changed {
            self.applied_typst_preference = self.settings.typst.clone();
            let next_typst = resolve_tool(ToolKind::Typst, &self.settings.typst);
            let typst_program_changed = next_typst.program != self.typst_tool.program;
            self.typst_tool = next_typst;
            if refresh_tools || typst_program_changed {
                self.schedule_compile_now();
            }
        }

        let tinymist_preference_changed =
            self.applied_tinymist_preference != self.settings.tinymist;
        let mut tinymist_program_changed = false;
        if refresh_tools || tinymist_preference_changed {
            self.applied_tinymist_preference = self.settings.tinymist.clone();
            let next_tinymist = resolve_tool(ToolKind::Tinymist, &self.settings.tinymist);
            tinymist_program_changed = next_tinymist.program != self.tinymist_tool.program;
            self.tinymist_tool = next_tinymist;
        }

        // A system appearance event changes the effective preview palette when
        // the document follows the interface, so refresh Tinymist as well as
        // the raster-page textures.
        if tinymist_restart_required(
            document_theme_mode_changed,
            preview_appearance_changed,
            preview_preference_changed,
            tinymist_program_changed,
            refresh_tools,
        ) {
            self.restart_tinymist();
        }
    }

    fn reload_active_theme(&mut self, context: &egui::Context) {
        let (active, error) = load_active_theme_or_fallback(&self.applied_theme_request);
        theme::set_imported_palette(Some((active.dark_mode, active.palette)));
        theme::configure_styles(context);
        self.generic_highlighter
            .set_custom_theme(Some(active.syntect_theme.clone()));
        self.highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::syntax_palette(active.dark_mode),
            Some(&active.syntect_theme),
            self.settings.typst_overrides.for_dark(active.dark_mode),
        ));
        self.applied_typst_overrides = self.settings.typst_overrides.clone();
        context.set_theme(active_theme_preference(
            self.settings.interface_theme,
            self.theme_override.is_some(),
            active.dark_mode,
        ));
        let name = active.name.as_deref().unwrap_or("Color theme");
        self.notice = Some(match error {
            Some(error) => Notice {
                message: format!("{error}; using {name}"),
                kind: NoticeKind::Error,
            },
            None => Notice {
                message: format!("Applied {name}"),
                kind: NoticeKind::Success,
            },
        });
        self.imported_theme = Some(active);
        context.request_repaint();
    }

    fn handle_dropped_file(&mut self, context: &egui::Context) {
        let path = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .find(|path| !path.as_os_str().is_empty() && (path.is_file() || path.is_dir()))
        });
        if let Some(path) = path {
            self.workspace_chooser_visible = false;
            self.queued_open_requests.push_back(path);
            context.request_repaint();
        }
    }

    fn receive_open_requests(&mut self, context: &egui::Context) {
        let incoming = self.open_requests.pending().collect::<Vec<_>>();
        if !incoming.is_empty() {
            self.workspace_chooser_visible = false;
            self.queued_open_requests.extend(incoming);
            context.request_repaint();
        }
        if self.document_flow_busy() {
            return;
        }
        if let Some(path) = self.queued_open_requests.pop_front() {
            self.queue_open_path(path);
        }
        if !self.queued_open_requests.is_empty() {
            context.request_repaint();
        }
    }

    fn queue_open_path(&mut self, path: PathBuf) {
        let path = canonical_or_absolute(&path);
        let (action, description) = if path.is_dir() {
            (
                DeferredDocumentAction::OpenFolder(path),
                "opening another workspace",
            )
        } else if path.is_file() {
            (
                DeferredDocumentAction::LoadPath(path),
                "opening another document",
            )
        } else {
            self.show_file_error(format!("{} does not exist", path.display()));
            return;
        };
        self.workspace_chooser_visible = false;
        self.request_document_replacement(action, description);
    }

    fn handle_close_request(&mut self, context: &egui::Context) {
        let close_requested = context.input(|input| input.viewport().close_requested());
        if close_requested && self.is_dirty() && !self.allow_close {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.app_modal.is_none() {
                self.request_document_replacement(
                    DeferredDocumentAction::CloseWindow,
                    "closing tiptoptyp",
                );
            }
        }
    }

    fn new_document(&mut self) {
        self.request_document_replacement(DeferredDocumentAction::New, "creating a new document");
    }

    fn reset_untitled_document(&mut self) {
        self.source = DEFAULT_SOURCE.to_owned();
        self.saved_source = self.source.clone();
        self.path = None;
        self.document_epoch = self.document_epoch.wrapping_add(1);
        self.disk_fingerprint = None;
        self.document_kind = DocumentKind::Typst;
        self.revision = self.revision.wrapping_add(1);
        self.autosave_deadline = None;
        self.reset_editor_history = true;
        self.editor_undo.clear();
        self.editor_redo.clear();
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.clear_preview_for_document();
        self.search.clear();
        self.reset_document_services();
        self.schedule_compile_now();
    }

    fn open_workspace_chooser(&mut self) {
        self.workspace_chooser_visible = true;
    }

    fn open_folder_dialog(&mut self) {
        self.request_document_replacement(
            DeferredDocumentAction::OpenFolderDialog,
            "opening another folder",
        );
    }

    fn native_file_dialog(&mut self, frame: &eframe::Frame) -> Option<AsyncFileDialog> {
        let dialog = AsyncFileDialog::new();

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(parent) = self.native_window_parent.as_ref() {
            return Some(dialog.set_parent(parent));
        }

        if self.window_host.is_root() {
            return Some(dialog.set_parent(frame));
        }

        self.show_file_error(
            "The active document window is not available yet; focus it and try again".to_owned(),
        );
        None
    }

    fn settings_file_dialog(&mut self, _frame: &eframe::Frame) -> Option<AsyncFileDialog> {
        let dialog = AsyncFileDialog::new();

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let Some(parent) = crate::native_window::active_window_handle() else {
                self.show_file_error(
                    "The Settings window is not active yet; focus it and try again".to_owned(),
                );
                return None;
            };
            Some(dialog.set_parent(&parent))
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Some(dialog.set_parent(_frame))
        }
    }

    fn start_open_folder_dialog(&mut self, frame: &eframe::Frame) {
        if self.pending_document_dialog.is_some() {
            return;
        }
        let Some(dialog) = self.native_file_dialog(frame) else {
            return;
        };
        let mut dialog = dialog.set_title("Open project folder");
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.pending_document_dialog = Some(PendingDocumentDialog {
            target: DocumentDialogTarget::OpenFolder,
            document_epoch: self.document_epoch,
            revision: self.revision,
            future: Box::pin(dialog.pick_folder()),
        });
    }

    fn finish_open_folder_selection(&mut self, folder: PathBuf) {
        let root = canonical_or_absolute(&folder);
        if !root.is_dir() {
            self.show_file_error(format!("{} is not a folder", root.display()));
            return;
        }
        self.workspace_root = root.clone();
        self.remember_workspace(&root);
        if self
            .path
            .as_ref()
            .is_some_and(|path| path.starts_with(&root))
        {
            if let Some(path) = self.path.clone() {
                self.remember_open_document(&path);
            }
            self.reset_document_services();
            if self.document_kind.is_typst() {
                self.schedule_compile_now();
            }
        } else if let Some(path) = remembered_document(&self.settings, &root) {
            if !self.load_path(path) {
                self.reset_untitled_document();
            }
        } else {
            self.reset_untitled_document();
        }
    }

    fn remember_workspace(&mut self, root: &Path) {
        self.settings.remember_workspace(root);
        if let Some(settings) = &mut self.pending_settings {
            settings.remember_workspace(root);
        }
    }

    fn open_dialog(&mut self) {
        self.request_document_replacement(
            DeferredDocumentAction::OpenFileDialog,
            "opening another document",
        );
    }

    fn open_in_new_window_dialog(&mut self, frame: &eframe::Frame) {
        if self.pending_document_dialog.is_some() {
            return;
        }
        let Some(dialog) = self.native_file_dialog(frame) else {
            return;
        };
        let mut dialog = dialog
            .add_filter(
                "Supported documents",
                &[
                    "typ", "pdf", "txt", "md", "rs", "toml", "json", "yaml", "yml", "xml", "html",
                    "css", "js", "ts", "py", "c", "cpp", "h", "png", "jpg", "jpeg", "gif", "webp",
                    "bmp", "ico", "tif", "tiff",
                ],
            )
            .add_filter("Typst documents", &["typ"])
            .add_filter("PDF documents", &["pdf"])
            .set_title("Open document in new window");
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.pending_document_dialog = Some(PendingDocumentDialog {
            target: DocumentDialogTarget::OpenFileInNewWindow,
            document_epoch: self.document_epoch,
            revision: self.revision,
            future: Box::pin(dialog.pick_file()),
        });
    }

    fn start_open_dialog(&mut self, frame: &eframe::Frame) {
        if self.pending_document_dialog.is_some() {
            return;
        }
        let Some(dialog) = self.native_file_dialog(frame) else {
            return;
        };
        let mut dialog = dialog
            .add_filter(
                "Supported documents",
                &[
                    "typ", "pdf", "txt", "md", "rs", "toml", "json", "yaml", "yml", "xml", "html",
                    "css", "js", "ts", "py", "c", "cpp", "h", "png", "jpg", "jpeg", "gif", "webp",
                    "bmp", "ico", "tif", "tiff",
                ],
            )
            .add_filter("Typst documents", &["typ"])
            .add_filter("PDF documents", &["pdf"])
            .add_filter(
                "Images",
                &[
                    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff",
                ],
            )
            .set_title("Open document");
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.pending_document_dialog = Some(PendingDocumentDialog {
            target: DocumentDialogTarget::OpenFile,
            document_epoch: self.document_epoch,
            revision: self.revision,
            future: Box::pin(dialog.pick_file()),
        });
    }

    fn load_path(&mut self, path: PathBuf) -> bool {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.show_file_error(format!("Could not open {}: {error}", path.display()));
                return false;
            }
        };
        let kind = match DocumentKind::detect(&path, &bytes) {
            Ok(kind) => kind,
            Err(error) => {
                self.show_file_error(error);
                return false;
            }
        };
        let source = if kind.is_editable() {
            // DocumentKind::detect already validated UTF-8.
            String::from_utf8(bytes.clone()).expect("validated UTF-8 document")
        } else {
            String::new()
        };
        let path = path.canonicalize().unwrap_or(path);
        if !path.starts_with(&self.workspace_root)
            && let Some(parent) = path.parent()
        {
            self.workspace_root = canonical_or_absolute(&discover_project_root(parent));
        }
        let workspace_root = self.workspace_root.clone();
        self.remember_workspace(&workspace_root);

        self.document_kind = kind;
        self.disk_fingerprint = kind.is_editable().then(|| fingerprint(&bytes));
        self.source = source;
        self.saved_source = self.source.clone();
        self.path = Some(path.clone());
        self.document_epoch = self.document_epoch.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        self.autosave_deadline = None;
        self.reset_editor_history = true;
        self.editor_undo.clear();
        self.editor_redo.clear();
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.remember_open_document(&path);
        self.clear_preview_for_document();
        self.search.clear();
        self.reset_document_services();

        if kind.is_typst() {
            self.schedule_compile_now();
        } else if kind.preview_only() {
            self.status = PreviewStatus::Compiling;
            if let Err(error) = self.asset_loader.request(self.asset_token, path, kind) {
                self.status = PreviewStatus::Error;
                self.notice = Some(Notice {
                    message: error,
                    kind: NoticeKind::Error,
                });
            }
        } else {
            self.status = PreviewStatus::Ready(Duration::ZERO);
        }
        true
    }

    fn save_document(&mut self, frame: &eframe::Frame) -> bool {
        if !self.document_kind.is_editable() {
            return true;
        }
        if let Some(path) = self.path.clone() {
            self.save_to(path)
        } else {
            self.save_as(frame)
        }
    }

    fn save_as(&mut self, frame: &eframe::Frame) -> bool {
        if !self.document_kind.is_editable() {
            return false;
        }
        if self.pending_document_dialog.is_some() {
            return false;
        }
        let typst = self.document_kind.is_typst();
        let Some(dialog) = self.native_file_dialog(frame) else {
            return false;
        };
        let mut dialog = dialog.set_file_name(self.document_name());
        dialog = if typst {
            dialog
                .add_filter("Typst documents", &["typ"])
                .set_title("Save Typst document")
        } else {
            dialog
                .add_filter("Text files", &["txt"])
                .set_title("Save text file")
        };
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.pending_document_dialog = Some(PendingDocumentDialog {
            target: DocumentDialogTarget::SaveAs { typst },
            document_epoch: self.document_epoch,
            revision: self.revision,
            future: Box::pin(dialog.save_file()),
        });
        false
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        self.save_to_with_intent(path, SaveIntent::Explicit)
    }

    fn save_to_with_intent(&mut self, path: PathBuf, intent: SaveIntent) -> bool {
        let path_changed = self
            .path
            .as_ref()
            .is_none_or(|current| !same_path(current, &path));
        if !path_changed {
            match intent {
                SaveIntent::Explicit if !self.confirm_disk_unchanged(&path) => return false,
                SaveIntent::ExplicitConfirmed => {}
                SaveIntent::Auto if !disk_matches_fingerprint(&path, self.disk_fingerprint) => {
                    self.notice = Some(Notice {
                        message: "Auto-save paused because the file changed on disk".to_owned(),
                        kind: NoticeKind::Error,
                    });
                    return false;
                }
                _ => {}
            }
        }
        match atomic_write(&self.project_root(), &path, self.source.as_bytes()) {
            Ok(()) => {
                self.path = Some(path.canonicalize().unwrap_or(path));
                if let Some(current) = self.path.as_deref()
                    && !current.starts_with(&self.workspace_root)
                    && let Some(parent) = current.parent()
                {
                    self.workspace_root = canonical_or_absolute(&discover_project_root(parent));
                }
                self.disk_fingerprint = Some(fingerprint(self.source.as_bytes()));
                if path_changed {
                    self.document_epoch = self.document_epoch.wrapping_add(1);
                    self.document_kind = self
                        .path
                        .as_deref()
                        .and_then(|path| DocumentKind::detect(path, self.source.as_bytes()).ok())
                        .filter(|kind| kind.is_editable())
                        .unwrap_or(DocumentKind::Text);
                    self.revision = self.revision.wrapping_add(1);
                    self.compiled_revision = None;
                    self.reset_document_services();
                    self.schedule_compile_now();
                } else {
                    self.refresh_workspace();
                    if self.document_kind.is_typst() {
                        // A saved subfile may be imported by the designated
                        // preview entry, so refresh the CLI fallback as well.
                        self.schedule_compile_now();
                    }
                }
                self.saved_source = self.source.clone();
                self.autosave_deadline = None;
                if let Some(path) = self.path.clone() {
                    self.remember_open_document(&path);
                }
                self.schedule_project_index();
                if let Some(mut action) = self.post_save_action.take() {
                    action.document_epoch = self.document_epoch;
                    action.revision = self.revision;
                    action.allow_discard = false;
                    self.pending_document_action = Some(action);
                }
                true
            }
            Err(error) => {
                match intent {
                    SaveIntent::Explicit | SaveIntent::ExplicitConfirmed => {
                        self.post_save_action = None;
                        self.show_file_error(error);
                    }
                    SaveIntent::Auto => {
                        self.notice = Some(Notice {
                            message: format!("Auto-save failed: {error}"),
                            kind: NoticeKind::Error,
                        });
                        self.autosave_deadline = Some(Instant::now() + AUTOSAVE_RETRY_DELAY);
                    }
                }
                false
            }
        }
    }

    fn confirm_disk_unchanged(&mut self, path: &Path) -> bool {
        let Some(expected) = self.disk_fingerprint else {
            return true;
        };
        let (description, observed_disk_fingerprint) = match fs::read(path) {
            Ok(contents) if fingerprint(&contents) == expected => return true,
            Ok(contents) => (
                format!(
                    "{} changed in another application. Overwrite those newer changes?",
                    path.display()
                ),
                Some(fingerprint(&contents)),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                format!(
                    "{} was deleted outside tiptoptyp. Create it again?",
                    path.display()
                ),
                None,
            ),
            Err(error) => {
                self.post_save_action = None;
                self.show_file_error(format!(
                    "Could not check {} before saving: {error}",
                    path.display()
                ));
                return false;
            }
        };

        self.app_modal = Some(AppModal::Overwrite {
            message: description,
            path: path.to_path_buf(),
            document_epoch: self.document_epoch,
            revision: self.revision,
            expected_disk_fingerprint: self.disk_fingerprint,
            observed_disk_fingerprint,
        });
        self.app_modal_had_focus = false;
        self.app_modal_suspended = false;
        false
    }

    fn export_pdf(&mut self, frame: &eframe::Frame) {
        if !matches!(self.document_kind, DocumentKind::Typst | DocumentKind::Pdf) {
            self.notice = Some(Notice {
                message: "PDF export is available for Typst and PDF documents".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        if self.pending_export_dialog.is_some() {
            return;
        }
        let export_source = if self.document_kind.is_typst() {
            self.designated_preview_path().or_else(|| self.path.clone())
        } else {
            self.path.clone()
        };
        let default_name = export_source
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| format!("{}.pdf", stem.to_string_lossy()))
            .unwrap_or_else(|| "document.pdf".to_owned());
        let Some(dialog) = self.native_file_dialog(frame) else {
            return;
        };
        let mut dialog = dialog
            .add_filter("PDF documents", &["pdf"])
            .set_title("Export PDF")
            .set_file_name(default_name);
        if let Some(directory) = export_source
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.current_directory())
        {
            dialog = dialog.set_directory(directory);
        }
        // NSSavePanel's synchronous `runModal` spins a nested AppKit loop from
        // inside winit's event callback. Dragging a folder across the panel can
        // then re-enter winit and trigger a non-unwinding panic. The async panel
        // uses a completion handler and is polled by normal egui frames instead.
        self.pending_export_dialog = Some(PendingExportDialog {
            document_epoch: self.document_epoch,
            future: Box::pin(dialog.save_file()),
        });
    }

    fn poll_export_dialog(&mut self, context: &egui::Context) {
        let Some(pending) = self.pending_export_dialog.as_mut() else {
            return;
        };
        let waker = Waker::from(Arc::new(EguiFutureWake(context.clone())));
        let mut task_context = TaskContext::from_waker(&waker);
        let Poll::Ready(selection) = pending.future.as_mut().poll(&mut task_context) else {
            context.request_repaint_after(Duration::from_millis(50));
            return;
        };
        let document_epoch = pending.document_epoch;
        self.pending_export_dialog = None;
        if let Some(file) = selection {
            if self.document_epoch == document_epoch {
                self.finish_export_selection(file.path().to_path_buf());
            } else {
                self.notice = Some(Notice {
                    message: "PDF export canceled because another document is now active"
                        .to_owned(),
                    kind: NoticeKind::Info,
                });
            }
        }
    }

    fn choose_tool_binary(
        &mut self,
        target: ToolPickerTarget,
        frame: &eframe::Frame,
        settings_context: &egui::Context,
    ) {
        if self.document_flow_busy() {
            self.notice = Some(Notice {
                message: "Finish the current file operation before browsing for a tool".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        let title = match target {
            ToolPickerTarget::Typst => "Choose Typst compiler",
            ToolPickerTarget::Tinymist => "Choose Tinymist language server",
            ToolPickerTarget::SublimeTheme { dark_mode: false } => {
                "Choose light Sublime color scheme"
            }
            ToolPickerTarget::SublimeTheme { dark_mode: true } => {
                "Choose dark Sublime color scheme"
            }
        };
        let Some(dialog) = self.settings_file_dialog(frame) else {
            return;
        };
        let mut dialog = dialog.set_title(title);
        if matches!(target, ToolPickerTarget::SublimeTheme { .. }) {
            dialog = dialog.add_filter("Sublime themes", &["tmTheme", "sublime-color-scheme"]);
        }
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.pending_tool_picker = Some(PendingToolPicker {
            target,
            future: Box::pin(dialog.pick_file()),
        });
        settings_context.request_repaint();
    }

    fn poll_tool_picker(&mut self, context: &egui::Context) {
        let Some(pending) = self.pending_tool_picker.as_mut() else {
            return;
        };
        let waker = Waker::from(Arc::new(EguiFutureWake(context.clone())));
        let mut task_context = TaskContext::from_waker(&waker);
        let Poll::Ready(selection) = pending.future.as_mut().poll(&mut task_context) else {
            context.request_repaint_after(Duration::from_millis(50));
            return;
        };
        let target = pending.target;
        self.pending_tool_picker = None;
        let Some(file) = selection else {
            return;
        };
        let mut edited = self
            .pending_settings
            .take()
            .unwrap_or_else(|| self.settings.clone());
        let value = file.path().display().to_string();
        match target {
            ToolPickerTarget::Typst => edited.typst.custom_path = value,
            ToolPickerTarget::Tinymist => edited.tinymist.custom_path = value,
            ToolPickerTarget::SublimeTheme {
                dark_mode: requested_dark,
            } => {
                let imported = match sublime_theme::import_path(file.path()) {
                    Ok(imported) => imported,
                    Err(error) => {
                        self.notice = Some(Notice {
                            message: format!("Could not import color scheme: {error}"),
                            kind: NoticeKind::Error,
                        });
                        self.pending_settings = Some(edited);
                        context.request_repaint();
                        return;
                    }
                };
                let inferred_dark = imported.dark_mode;
                *edited.color_theme_mut(if inferred_dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                }) = ColorThemeChoice::sublime(value);
                let inferred = if inferred_dark { "dark" } else { "light" };
                let name = imported.name.as_deref().unwrap_or("Sublime color scheme");
                self.notice = Some(Notice {
                    message: if inferred_dark == requested_dark {
                        format!("Imported {name} for {inferred} appearance")
                    } else {
                        format!("Imported {name} as the {inferred} theme based on its palette")
                    },
                    kind: NoticeKind::Success,
                });
            }
        }
        self.pending_settings = Some(edited);
        context.request_repaint();
    }

    fn poll_document_dialog(&mut self, context: &egui::Context) {
        let Some(pending) = self.pending_document_dialog.as_mut() else {
            return;
        };
        let waker = Waker::from(Arc::new(EguiFutureWake(context.clone())));
        let mut task_context = TaskContext::from_waker(&waker);
        let Poll::Ready(selection) = pending.future.as_mut().poll(&mut task_context) else {
            context.request_repaint_after(Duration::from_millis(50));
            return;
        };
        let target = pending.target;
        let document_epoch = pending.document_epoch;
        let revision = pending.revision;
        self.pending_document_dialog = None;
        let Some(file) = selection else {
            self.post_save_action = None;
            self.schedule_autosave_if_needed();
            return;
        };
        let mut path = file.path().to_path_buf();
        match target {
            DocumentDialogTarget::OpenFile => {
                if self.document_epoch != document_epoch {
                    self.notice = Some(Notice {
                        message: "Open canceled because another document is now active".to_owned(),
                        kind: NoticeKind::Info,
                    });
                } else if self.revision != revision {
                    self.request_document_replacement(
                        DeferredDocumentAction::LoadPath(path),
                        "opening the selected document",
                    );
                } else {
                    self.load_path(path);
                }
                self.schedule_autosave_if_needed();
            }
            DocumentDialogTarget::OpenFileInNewWindow => {
                self.pending_window_requests
                    .push_back(EditorWindowRequest::Open(path));
            }
            DocumentDialogTarget::OpenFolder => {
                if self.document_epoch != document_epoch {
                    self.notice = Some(Notice {
                        message: "Open Folder canceled because another document is now active"
                            .to_owned(),
                        kind: NoticeKind::Info,
                    });
                } else if self.revision != revision {
                    self.request_document_replacement(
                        DeferredDocumentAction::OpenFolder(path),
                        "opening the selected folder",
                    );
                } else {
                    self.finish_open_folder_selection(path);
                }
                self.schedule_autosave_if_needed();
            }
            DocumentDialogTarget::SaveAs { typst } => {
                if self.document_epoch != document_epoch {
                    self.post_save_action = None;
                    self.notice = Some(Notice {
                        message: "Save As was canceled because another document is now open"
                            .to_owned(),
                        kind: NoticeKind::Error,
                    });
                    self.schedule_autosave_if_needed();
                    return;
                }
                if typst && path.extension().is_none() {
                    path.set_extension("typ");
                }
                self.save_to(path);
            }
        }
    }

    fn finish_export_selection(&mut self, mut path: PathBuf) {
        if path.extension().is_none() {
            path.set_extension("pdf");
        }

        if self.compiled_revision == Some(self.revision)
            && let Some(pdf) = self.pdf.as_deref()
        {
            if let Err(error) = atomic_write(&self.project_root(), &path, pdf) {
                self.show_file_error(error);
            } else {
                self.notice = Some(Notice {
                    message: format!("Exported {}", path.display()),
                    kind: NoticeKind::Success,
                });
            }
            return;
        }

        self.pending_export = Some(PendingExport {
            path,
            document_epoch: self.document_epoch,
        });
        self.notice = Some(Notice {
            message: "Export queued for the next successful build".to_owned(),
            kind: NoticeKind::Info,
        });
        if self.document_kind.is_typst() {
            self.schedule_compile_now();
        }
    }

    fn complete_pending_export(&mut self) {
        let (Some(path), Some(pdf)) = (
            take_matching_export(&mut self.pending_export, self.document_epoch),
            self.pdf.as_deref(),
        ) else {
            return;
        };
        if let Err(error) = atomic_write(&self.project_root(), &path, pdf) {
            self.show_file_error(error);
        } else {
            self.notice = Some(Notice {
                message: format!("Exported {}", path.display()),
                kind: NoticeKind::Success,
            });
        }
    }

    fn request_document_replacement(&mut self, action: DeferredDocumentAction, description: &str) {
        if self.document_flow_busy() {
            return;
        }
        let pending = PendingDocumentAction {
            action,
            document_epoch: self.document_epoch,
            revision: self.revision,
            allow_discard: false,
            description: description.to_owned(),
        };
        if self.is_dirty() {
            self.present_unsaved_prompt(pending);
        } else {
            self.pending_document_action = Some(pending);
        }
    }

    fn present_unsaved_prompt(&mut self, mut pending: PendingDocumentAction) {
        pending.document_epoch = self.document_epoch;
        pending.revision = self.revision;
        pending.allow_discard = false;
        self.app_modal = Some(AppModal::Unsaved {
            message: format!(
                "Save changes to {} before {}?",
                self.document_name(),
                pending.description
            ),
            pending,
        });
        self.app_modal_had_focus = false;
        self.app_modal_suspended = false;
    }

    fn execute_pending_document_action(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        let Some(mut pending) = self.pending_document_action.take() else {
            return;
        };
        if pending.document_epoch != self.document_epoch {
            self.post_save_action = None;
            self.notice = Some(Notice {
                message: "Canceled a stale document action because another document is open"
                    .to_owned(),
                kind: NoticeKind::Info,
            });
            self.schedule_autosave_if_needed();
            return;
        }

        match pending.action {
            DeferredDocumentAction::SaveThen(action) => {
                self.post_save_action = Some(*action);
                if self.save_document(frame)
                    && let Some(mut action) = self.post_save_action.take()
                {
                    action.document_epoch = self.document_epoch;
                    action.revision = self.revision;
                    action.allow_discard = false;
                    self.pending_document_action = Some(action);
                }
                return;
            }
            DeferredDocumentAction::ForceSave {
                path,
                document_epoch,
                revision,
                expected_disk_fingerprint,
                observed_disk_fingerprint,
            } => {
                let same_document = document_epoch == self.document_epoch
                    && self
                        .path
                        .as_ref()
                        .is_some_and(|current| same_path(current, &path))
                    && self.disk_fingerprint == expected_disk_fingerprint;
                if !same_document {
                    self.post_save_action = None;
                    self.notice = Some(Notice {
                        message: "Overwrite canceled because the document changed".to_owned(),
                        kind: NoticeKind::Error,
                    });
                    self.schedule_autosave_if_needed();
                    return;
                }
                let current_disk_fingerprint = match fs::read(&path) {
                    Ok(contents) => Some(fingerprint(&contents)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => {
                        self.post_save_action = None;
                        self.show_file_error(format!(
                            "Could not recheck {} before saving: {error}",
                            path.display()
                        ));
                        return;
                    }
                };
                if revision != self.revision
                    || current_disk_fingerprint != observed_disk_fingerprint
                {
                    self.save_to_with_intent(path, SaveIntent::Explicit);
                } else {
                    self.save_to_with_intent(path, SaveIntent::ExplicitConfirmed);
                }
                return;
            }
            action => pending.action = action,
        }

        if pending.allow_discard {
            if pending.revision != self.revision {
                self.present_unsaved_prompt(pending);
                return;
            }
        } else if self.is_dirty() {
            self.present_unsaved_prompt(pending);
            return;
        }

        match pending.action {
            DeferredDocumentAction::New => self.reset_untitled_document(),
            DeferredDocumentAction::OpenFileDialog => self.start_open_dialog(frame),
            DeferredDocumentAction::OpenFolderDialog => self.start_open_folder_dialog(frame),
            DeferredDocumentAction::CloseWindow => {
                self.allow_close = true;
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            DeferredDocumentAction::LoadPath(path) => {
                self.load_path(path);
            }
            DeferredDocumentAction::OpenFolder(path) => {
                self.finish_open_folder_selection(path);
            }
            DeferredDocumentAction::FollowFileLink {
                path,
                page,
                source_position,
            } => {
                if self.load_path(path) {
                    self.apply_file_link_location(page, source_position);
                }
            }
            DeferredDocumentAction::FollowTinymistLocation { path, selection } => {
                if self.load_path(path) {
                    self.apply_tinymist_selection(selection.as_ref());
                }
            }
            DeferredDocumentAction::SaveThen(_) | DeferredDocumentAction::ForceSave { .. } => {
                unreachable!("save actions are handled before replacement validation")
            }
        }
        self.schedule_autosave_if_needed();
    }

    fn show_file_error(&mut self, message: String) {
        self.notice = Some(Notice {
            message: message.clone(),
            kind: NoticeKind::Error,
        });
        self.app_modal = Some(AppModal::Alert {
            title: "error".to_owned(),
            message,
            kind: NoticeKind::Error,
        });
        self.app_modal_had_focus = false;
        self.app_modal_suspended = false;
    }

    fn reset_document_services(&mut self) {
        let root = self.project_root();
        self.workspace = None;
        self.workspace_error = None;
        self.request_workspace_scan(root);
        self.next_workspace_refresh = Instant::now() + WORKSPACE_REFRESH_INTERVAL;
        self.restart_tinymist();
    }

    fn request_workspace_scan(&mut self, root: PathBuf) {
        self.workspace_scan_id = self.workspace_scan_id.wrapping_add(1);
        let scan_id = self.workspace_scan_id;
        let (sender, receiver) = mpsc::channel();
        self.workspace_scan = Some(receiver);
        let _ = thread::Builder::new()
            .name("tiptoptyp-workspace-scan".to_owned())
            .spawn(move || {
                let result = WorkspaceSnapshot::scan(&root).map_err(|error| error.to_string());
                let _ = sender.send((scan_id, result));
            });
    }

    fn poll_workspace_scan(&mut self) {
        let Some(receiver) = self.workspace_scan.as_ref() else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.workspace_scan = None;
                return;
            }
        };
        self.workspace_scan = None;
        let (scan_id, result) = result;
        if scan_id != self.workspace_scan_id {
            return;
        }
        match result {
            Ok(snapshot) => {
                if let Some(workspace) = &mut self.workspace {
                    workspace.apply_snapshot(snapshot);
                } else {
                    self.workspace = Some(WorkspaceTree::from_snapshot(snapshot));
                }
                self.workspace_error = None;
            }
            Err(error) => {
                self.workspace_error = Some(format!("Could not scan workspace: {error}"));
            }
        }
    }

    fn refresh_workspace(&mut self) {
        if self.workspace_scan.is_none() {
            let root = self
                .workspace
                .as_ref()
                .map(|workspace| workspace.root().to_owned())
                .unwrap_or_else(|| self.project_root());
            self.request_workspace_scan(root);
        }
        self.next_workspace_refresh = Instant::now() + WORKSPACE_REFRESH_INTERVAL;
    }

    fn tick_workspace(&mut self, context: &egui::Context) {
        self.poll_workspace_scan();
        if self.workspace_scan.is_some() {
            context.request_repaint_after(Duration::from_millis(50));
        }
        if !self.filesystem_visible {
            return;
        }
        let now = Instant::now();
        if now >= self.next_workspace_refresh {
            self.refresh_workspace();
        }
        context.request_repaint_after(self.next_workspace_refresh.saturating_duration_since(now));
    }

    fn tinymist_document_path(&self) -> PathBuf {
        self.tinymist_unsaved_document
            .as_ref()
            .map(|document| document.path().to_owned())
            .or_else(|| self.path.clone())
            .unwrap_or_else(|| self.preview_document_path())
    }

    fn restart_tinymist(&mut self) {
        let start_preview = self.interactive_preview_requested();
        self.tinymist_preview_enabled = start_preview;
        if let Some(generation) = self.tinymist_generation.take() {
            let current_uri = self.tinymist_uri.take();
            let preview_uri = self.tinymist_preview_uri.take();
            if self.tinymist_current_open
                && let Some(uri) = current_uri.as_ref()
            {
                let _ = self.tinymist.did_close(generation, uri.clone());
            }
            if let Some(uri) = preview_uri
                && current_uri.as_deref() != Some(uri.as_str())
            {
                let _ = self.tinymist.did_close(generation, uri);
            }
            let _ = self.tinymist.stop_workspace(generation);
        }
        self.tinymist_current_open = false;
        // Keep the private backing alive through didClose, then clean it.
        self.tinymist_unsaved_document.take();
        self.tinymist_url = None;
        self.tinymist_lsp_ready = false;
        self.tinymist_diagnostics.clear();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.webview = None;
            self.webview_url = None;
        }
        if !self.document_kind.is_typst() {
            self.tinymist_state =
                ServiceState::Disabled("The selected file is not a Typst document".to_owned());
            self.webview_state =
                ServiceState::Disabled("Binary files use the native preview".to_owned());
            return;
        }
        if !self.tinymist_tool.is_available() {
            let reason = self
                .tinymist_tool
                .fallback_reason
                .clone()
                .unwrap_or_else(|| "Tinymist is unavailable".to_owned());
            self.tinymist_state = ServiceState::Failed(reason.clone());
            self.webview_state = ServiceState::Failed(reason);
            return;
        }
        self.tinymist_state = ServiceState::Starting("Launching Tinymist LSP".to_owned());
        self.webview_state = if self.settings.preview_preference == PreviewPreference::Native {
            ServiceState::Disabled("Interactive preview is not requested".to_owned())
        } else if !cfg!(any(target_os = "macos", target_os = "windows")) {
            ServiceState::Unsupported(
                "Embedded Tinymist preview is currently available on macOS and Windows".to_owned(),
            )
        } else if !start_preview {
            ServiceState::Disabled(
                "Interactive preview is paused while Code view is active".to_owned(),
            )
        } else {
            ServiceState::Starting("Waiting for Tinymist's preview server".to_owned())
        };
        let mut config = TinymistConfig::new(self.project_root())
            .with_executable(self.tinymist_tool.program.clone());
        config.start_preview = start_preview;
        config.preview.invert_colors =
            tinymist_invert_colors(self.settings.document_theme, self.preview_dark);
        match self.tinymist.start_workspace(config) {
            Ok(generation) => {
                let version = revision_as_i32(self.revision);
                let current_document = if let Some(path) = self.path.as_deref() {
                    TextDocument::from_path(path, version, self.source.clone())
                } else {
                    let source_dir = self
                        .current_directory()
                        .unwrap_or_else(|| self.project_root());
                    match UnsavedTextDocument::create(
                        self.project_root(),
                        source_dir,
                        self.document_name(),
                        &self.source,
                    ) {
                        Ok(document) => {
                            let text_document =
                                document.text_document(version, self.source.clone());
                            self.tinymist_unsaved_document = Some(document);
                            Ok(text_document)
                        }
                        Err(error) => Err(error),
                    }
                };
                let current_document = match current_document {
                    Ok(document) => document,
                    Err(error) => {
                        let _ = self.tinymist.stop_workspace(generation);
                        self.tinymist_state = ServiceState::Failed(error.to_string());
                        return;
                    }
                };

                let preview_document = if self.current_is_preview_document() {
                    current_document.clone()
                } else {
                    let path = self.preview_document_path();
                    match self.preview_document_source().and_then(|source| {
                        TextDocument::from_path(&path, version, source)
                            .map_err(|error| error.to_string())
                    }) {
                        Ok(document) => document,
                        Err(error) => {
                            let _ = self.tinymist.stop_workspace(generation);
                            self.tinymist_state = ServiceState::Failed(error);
                            return;
                        }
                    }
                };

                self.tinymist_uri = Some(current_document.uri.clone());
                self.tinymist_preview_uri = Some(preview_document.uri.clone());
                self.tinymist_generation = Some(generation);
                self.tinymist_current_open = current_document.uri == preview_document.uri;
                // The first document opened determines startDefaultPreview.
                // Imported/current subfiles are opened after Initialized.
                if let Err(error) = self.tinymist.did_open(generation, preview_document) {
                    self.tinymist_state = ServiceState::Failed(error.to_string());
                }
            }
            Err(error) => {
                self.tinymist_state = ServiceState::Failed(error.to_string());
            }
        }
    }

    fn sync_tinymist_change(&mut self) {
        if !self.document_kind.is_typst() {
            return;
        }
        if let Some(document) = &self.tinymist_unsaved_document
            && let Err(error) = document.update_backing_source(&self.source)
        {
            self.tinymist_state = ServiceState::Degraded(error.to_string());
            return;
        }
        let (Some(generation), Some(uri)) = (self.tinymist_generation, self.tinymist_uri.clone())
        else {
            return;
        };
        if !self.tinymist_current_open {
            // Initialized opens this document using the newest buffer.
            return;
        }
        if let Err(error) = self.tinymist.did_change(
            generation,
            uri,
            revision_as_i32(self.revision),
            self.source.clone(),
        ) {
            self.tinymist_state = ServiceState::Degraded(error.to_string());
        }
    }

    fn receive_tinymist_events(&mut self, context: &egui::Context) {
        while let Some(event) = self.tinymist.try_recv() {
            if !self.document_kind.is_typst() {
                continue;
            }
            match event {
                TinymistEvent::Starting { .. } => {
                    self.tinymist_lsp_ready = false;
                    self.tinymist_state =
                        ServiceState::Starting("Launching Tinymist LSP".to_owned());
                }
                TinymistEvent::Initialized { generation } => {
                    self.tinymist_lsp_ready = true;
                    self.tinymist_state = if self.interactive_preview_requested() {
                        ServiceState::Starting("Starting Tinymist preview server".to_owned())
                    } else {
                        ServiceState::Ready("Tinymist LSP is ready".to_owned())
                    };
                    if !self.tinymist_current_open
                        && self.tinymist_generation == Some(generation)
                        && let Some(uri) = self.tinymist_uri.clone()
                    {
                        let document = TextDocument::typst(
                            uri,
                            revision_as_i32(self.revision),
                            self.source.clone(),
                        );
                        match self.tinymist.did_open(generation, document) {
                            Ok(()) => self.tinymist_current_open = true,
                            Err(error) => {
                                self.tinymist_state = ServiceState::Degraded(error.to_string())
                            }
                        }
                    }
                }
                TinymistEvent::PreviewReady { url, .. } => {
                    self.tinymist_state =
                        ServiceState::Ready("LSP and preview server are ready".to_owned());
                    if self.interactive_preview_requested() {
                        self.tinymist_url = Some(url);
                        self.webview_state =
                            ServiceState::Starting("Embedding the vector preview".to_owned());
                    }
                }
                TinymistEvent::ShowDocument { uri, selection, .. } => {
                    self.follow_tinymist_location(&uri, selection.as_ref());
                }
                TinymistEvent::PublishDiagnostics {
                    uri,
                    version,
                    diagnostics,
                    ..
                } => {
                    self.receive_tinymist_diagnostics(&uri, version, diagnostics);
                }
                TinymistEvent::Formatted {
                    generation,
                    uri,
                    version,
                    edits,
                } => {
                    self.receive_formatted_document(context, generation, &uri, version, edits);
                }
                TinymistEvent::Hovered {
                    uri,
                    version,
                    request_token,
                    contents,
                    ..
                } => {
                    if let Some(hover) = &mut self.editor_hover
                        && hover.request_token == request_token
                        && hover.version == version
                        && hover.uri == uri
                    {
                        hover.detail = contents;
                    }
                }
                TinymistEvent::Error {
                    stage,
                    message,
                    fatal,
                    ..
                } => {
                    if stage == "formatting" && !fatal {
                        self.notice = Some(Notice {
                            message: format!("Formatting failed: {message}"),
                            kind: NoticeKind::Error,
                        });
                        continue;
                    }
                    let detail = format!("{stage}: {message}");
                    self.tinymist_state = if fatal {
                        self.tinymist_lsp_ready = false;
                        self.tinymist_url = None;
                        self.webview_state = ServiceState::Failed(
                            "Tinymist stopped before the embedded preview was available".to_owned(),
                        );
                        ServiceState::Failed(detail)
                    } else {
                        ServiceState::Degraded(detail)
                    };
                }
                TinymistEvent::Stopped { reason, .. } => {
                    self.tinymist_lsp_ready = false;
                    self.tinymist_url = None;
                    let detail = match &self.tinymist_state {
                        ServiceState::Failed(previous) if previous != &reason => {
                            format!("{previous}; {reason}")
                        }
                        _ => reason,
                    };
                    self.tinymist_state = ServiceState::Failed(detail);
                    self.webview_state =
                        ServiceState::Failed("Tinymist preview is no longer running".to_owned());
                }
                _ => {}
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn receive_web_links(&mut self) {
        let targets = self.web_link_receiver.try_iter().collect::<Vec<_>>();
        for target in targets {
            self.handle_web_link(&target);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn receive_web_links(&mut self) {}

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn handle_web_link(&mut self, target: &str) {
        self.follow_preview_link(target);
    }

    fn follow_preview_link(&mut self, target: &str) {
        if let Some(page) = internal_pdf_page_target(target) {
            self.requested_page = Some(page.min(self.pages.len().saturating_sub(1)));
            return;
        }

        let target = if target.starts_with("www.") {
            format!("https://{target}")
        } else {
            target.to_owned()
        };
        let url = match url::Url::parse(&target) {
            Ok(url) => url,
            Err(_) => {
                let Some(directory) = self.current_directory() else {
                    self.notice = Some(Notice {
                        message: format!("Could not resolve link: {target}"),
                        kind: NoticeKind::Error,
                    });
                    return;
                };
                let Ok(base) = url::Url::from_directory_path(directory) else {
                    return;
                };
                let Ok(url) = base.join(&target) else {
                    self.notice = Some(Notice {
                        message: format!("Could not follow malformed link: {target}"),
                        kind: NoticeKind::Error,
                    });
                    return;
                };
                url
            }
        };

        if url.scheme() == "file" {
            self.follow_file_link(&url);
            return;
        }

        if matches!(url.scheme(), "http" | "https" | "mailto" | "tel") {
            if let Err(error) = open_in_system_browser(url.as_str()) {
                self.notice = Some(Notice {
                    message: error,
                    kind: NoticeKind::Error,
                });
            }
        } else {
            self.notice = Some(Notice {
                message: format!("Unsupported link scheme: {}", url.scheme()),
                kind: NoticeKind::Error,
            });
        }
    }

    fn follow_file_link(&mut self, url: &url::Url) {
        let page = pdf_page_from_url(url);
        let source_position = source_position_from_url(url);
        let Ok(path) = url.to_file_path() else {
            return;
        };
        if !DocumentKind::supports_path(&path) && path.extension().is_some() {
            self.notice = Some(Notice {
                message: format!("Unsupported linked file: {}", path.display()),
                kind: NoticeKind::Error,
            });
            return;
        }
        let same_document = self
            .path
            .as_ref()
            .is_some_and(|current| same_path(current, &path));
        if !same_document {
            self.request_document_replacement(
                DeferredDocumentAction::FollowFileLink {
                    path,
                    page,
                    source_position,
                },
                "following a document link",
            );
            return;
        }
        self.apply_file_link_location(page, source_position);
    }

    fn apply_file_link_location(
        &mut self,
        page: Option<usize>,
        source_position: Option<(usize, usize)>,
    ) {
        if self.document_kind == DocumentKind::Pdf {
            if self.pages.is_empty() {
                self.pending_asset_page = page;
            } else if let Some(page) = page {
                self.requested_page = Some(page.min(self.pages.len().saturating_sub(1)));
            }
        } else if self.document_kind.is_editable()
            && let Some((line, column)) = source_position
        {
            let char_index = char_index_at_line_column(&self.source, line, column);
            self.pending_editor_selection = Some(char_index..char_index);
            self.editor_attention = Some(EditorAttention {
                char_index,
                started: Instant::now(),
            });
            if self.document_kind.is_typst() {
                self.view_mode = ViewMode::Split;
            }
        }
    }

    fn follow_tinymist_location(&mut self, uri: &str, selection: Option<&LspRange>) {
        let Ok(url) = url::Url::parse(uri) else {
            return;
        };
        let Ok(path) = url.to_file_path() else {
            return;
        };
        let virtual_untitled = self.path.is_none() && path == self.tinymist_document_path();
        let same_document = virtual_untitled
            || self
                .path
                .as_ref()
                .is_some_and(|current| same_path(current, &path));
        if !same_document {
            if path.extension().is_none_or(|extension| extension != "typ") {
                return;
            }
            self.request_document_replacement(
                DeferredDocumentAction::FollowTinymistLocation {
                    path,
                    selection: selection.cloned(),
                },
                "following the preview location",
            );
            return;
        }
        self.apply_tinymist_selection(selection);
    }

    fn apply_tinymist_selection(&mut self, selection: Option<&LspRange>) {
        if let Some(selection) = selection {
            let range = range_to_char_range(&self.source, selection);
            self.editor_attention = Some(EditorAttention {
                char_index: range.start,
                started: Instant::now(),
            });
            self.pending_editor_selection = Some(range);
        }
        self.view_mode = ViewMode::Split;
    }

    fn jump_source_to_preview(&mut self, char_index: usize) {
        if !self.document_kind.is_typst() || !self.interactive_preview_active() {
            return;
        }
        let Some(generation) = self.tinymist_generation else {
            return;
        };
        let (line, character) = scalar_position_at_char(&self.source, char_index);
        let path = self.tinymist_document_path();
        if let Err(error) = self
            .tinymist
            .scroll_preview(generation, path, line, character)
        {
            self.notice = Some(Notice {
                message: format!("Could not reveal source in preview: {error}"),
                kind: NoticeKind::Error,
            });
            return;
        }
        if self.view_mode == ViewMode::Code {
            self.view_mode = ViewMode::Split;
        }
    }

    fn preview_visible(&self) -> bool {
        self.document_kind.preview_only()
            || (self.document_kind.is_typst() && self.view_mode.shows_preview())
    }

    fn interactive_preview_requested(&self) -> bool {
        self.document_kind.is_typst()
            && self.settings.preview_preference == PreviewPreference::Interactive
            && cfg!(any(target_os = "macos", target_os = "windows"))
    }

    fn preview_processing_enabled(&self) -> bool {
        self.document_kind.is_typst()
    }

    fn sync_preview_visibility(&mut self) {
        let visible = self.preview_visible();
        let became_visible = !self.preview_was_visible && visible;
        self.preview_was_visible = visible;

        if !visible {
            self.hide_webview();
        }

        if self.tinymist_preview_enabled != self.interactive_preview_requested() {
            self.restart_tinymist();
        }
        if became_visible {
            self.schedule_compile_now();
        }
    }

    fn receive_tinymist_diagnostics(
        &mut self,
        uri: &str,
        version: Option<i32>,
        diagnostics: Vec<TinymistDiagnostic>,
    ) {
        if self.tinymist_uri.as_deref() == Some(uri)
            && version.is_some_and(|version| version != revision_as_i32(self.revision))
        {
            return;
        }
        let source = if self.tinymist_preview_uri.as_deref() == Some(uri) {
            DiagnosticSource::Main
        } else {
            url::Url::parse(uri)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .map_or(DiagnosticSource::Global, DiagnosticSource::File)
        };
        let mut converted = diagnostics
            .into_iter()
            .map(|diagnostic| tinymist_diagnostic(diagnostic, source.clone()))
            .collect();
        normalize_diagnostics(&mut converted);
        self.tinymist_diagnostics = converted;
    }

    fn interactive_preview_active(&self) -> bool {
        self.interactive_preview_requested()
            && self.tinymist_url.is_some()
            && self.webview_state.is_ready()
    }

    fn should_attempt_interactive_preview(&self) -> bool {
        self.interactive_preview_requested()
            && self.tinymist_url.is_some()
            && !matches!(
                self.webview_state,
                ServiceState::Failed(_) | ServiceState::Unsupported(_)
            )
            && cfg!(any(target_os = "macos", target_os = "windows"))
    }

    fn interactive_preview_transitioning(&self) -> bool {
        self.interactive_preview_requested()
            && matches!(
                self.tinymist_state,
                ServiceState::Starting(_) | ServiceState::Ready(_)
            )
            && !matches!(
                self.webview_state,
                ServiceState::Failed(_) | ServiceState::Unsupported(_)
            )
    }

    fn preview_fallback_reason(&self) -> Option<String> {
        if !self.document_kind.is_typst() || !self.preview_visible() {
            return None;
        }
        preview_fallback_reason_for(
            self.settings.preview_preference,
            self.interactive_preview_active(),
            self.tinymist_url.is_some(),
            &self.tinymist_state,
            &self.webview_state,
        )
    }

    fn preview_backend_label(&self) -> &'static str {
        if !self.document_kind.is_typst() {
            return match self.document_kind {
                DocumentKind::Pdf => "Rasterised PDF",
                DocumentKind::Image => "Image",
                DocumentKind::Text => "Text editor",
                DocumentKind::Typst => unreachable!(),
            };
        }
        preview_backend_label_for(
            self.settings.preview_preference,
            self.interactive_preview_active(),
        )
    }

    fn non_preview_fallback_details(&self, system_theme: Option<egui::Theme>) -> Vec<String> {
        non_preview_fallback_details_for(
            &self.settings,
            system_theme,
            &self.typst_tool,
            &self.tinymist_tool,
        )
    }

    fn rebuild_preview_textures(&mut self, context: &egui::Context) {
        let revision = self.compiled_revision.unwrap_or(self.revision);
        let dark = self.preview_dark && self.document_kind != DocumentKind::Image;
        for (index, page) in self.pages.iter_mut().enumerate() {
            let pixels = if dark {
                dark_preview_rgba(&page.rgba)
            } else {
                page.rgba.clone()
            };
            page.texture = context.load_texture(
                format!("preview-{revision}-{index}-{dark}"),
                preview_color_image(page.raster_size, &pixels),
                TextureOptions::LINEAR,
            );
        }
    }

    fn show_toolbar(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        #[cfg(target_os = "macos")]
        {
            // The toolbar occupies the native title-bar row. Controls start to
            // the right of the traffic lights, while empty toolbar space stays
            // draggable.
            let drag = ui.interact(
                ui.max_rect(),
                ui.id().with("window-title-drag"),
                Sense::drag(),
            );
            if drag.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        }

        ui.horizontal_centered(|ui| {
            theme::apply_compact_control_spacing(ui);

            #[cfg(target_os = "macos")]
            {
                use raw_window_handle::HasWindowHandle as _;

                let traffic_lights_width = self
                    .window_host
                    .is_root()
                    .then(|| frame.window_handle().ok())
                    .flatten()
                    .and_then(|handle| {
                        eframe::WindowChromeMetrics::from_window_handle(&handle.as_raw())
                    })
                    .map(|metrics| metrics.traffic_lights_size.x / ui.ctx().zoom_factor().max(0.1))
                    .unwrap_or(METRICS.toolbar.traffic_lights_fallback_width);
                ui.add_space(traffic_lights_width + METRICS.toolbar.traffic_lights_gap);
            }

            let toolbar_width = ui.available_width();
            let compact = toolbar_width < METRICS.toolbar.compact_breakpoint;
            if compact {
                theme::apply_dense_toolbar_spacing(ui);
            }

            let title = format!(
                "{}{}",
                self.document_name(),
                if self.is_dirty() { "*" } else { "" }
            );
            let natural_title_width = title.chars().count() as f32
                * METRICS.toolbar.title_character_width
                + METRICS.toolbar.title_padding;
            let title_width = if compact {
                natural_title_width.clamp(
                    METRICS.toolbar.compact_title_min,
                    METRICS.toolbar.compact_title_max,
                )
            } else {
                natural_title_width.clamp(METRICS.toolbar.title_min, METRICS.toolbar.title_max)
            };
            let title_response = ui.add_sized(
                [title_width, METRICS.toolbar.title_height],
                egui::Label::new(RichText::new(&title).strong())
                    .truncate()
                    .sense(Sense::click()),
            );
            native_hover_text(
                title_response.clone(),
                format!(
                    "{}\nDouble-click to rename",
                    self.path.as_ref().map_or_else(
                        || "Unsaved document".to_owned(),
                        |path| path.display().to_string()
                    )
                ),
            );
            if title_response.double_clicked() && !self.document_flow_busy() {
                if let Some(path) = self.path.clone() {
                    self.begin_rename(path);
                } else {
                    self.save_as(frame);
                }
            }
            ui.separator();

            self.show_file_menu(ui);
            self.show_edit_menu(ui);
            ui.separator();
            if native_hover_text(ui.button("Find"), "Find · Cmd/Ctrl+F").clicked() {
                self.open_find(false);
            }
            if native_hover_text(
                ui.add_enabled(self.document_kind.is_typst(), egui::Button::new("Compile")),
                "Compile now · Cmd/Ctrl+R",
            )
            .clicked()
            {
                self.request_compile();
            }

            // Consuming the remaining width with a right-to-left layout keeps
            // the view controls pinned to the opposite edge while the title is
            // the only item that contracts at narrow window sizes.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                native_hover_text(
                    ui.selectable_value(
                        &mut self.view_mode,
                        ViewMode::Preview,
                        if compact { "P" } else { "Preview" },
                    ),
                    "Preview",
                );
                native_hover_text(
                    ui.selectable_value(
                        &mut self.view_mode,
                        ViewMode::Split,
                        if compact { "S" } else { "Split" },
                    ),
                    "Split",
                );
                native_hover_text(
                    ui.selectable_value(
                        &mut self.view_mode,
                        ViewMode::Code,
                        if compact { "C" } else { "Code" },
                    ),
                    "Code",
                );
                let explorer_label = if compact { "Files" } else { "Explorer" };
                if native_hover_text(
                    ui.selectable_label(self.filesystem_visible, explorer_label),
                    "Toggle the file explorer",
                )
                .clicked()
                {
                    self.filesystem_visible = !self.filesystem_visible;
                }
                if native_hover_text(
                    ui.selectable_label(
                        self.problems_visible,
                        if compact { "!" } else { "Problems" },
                    ),
                    "Toggle compiler diagnostics",
                )
                .clicked()
                {
                    self.problems_visible = !self.problems_visible;
                }
                if native_hover_text(
                    ui.selectable_label(
                        self.settings_visible,
                        if compact { "Set" } else { "Settings" },
                    ),
                    "Open Settings in a separate window",
                )
                .clicked()
                {
                    self.settings_visible = !self.settings_visible;
                }
            });
        });
    }

    fn show_file_menu(&mut self, ui: &mut egui::Ui) {
        let selected = matches!(self.app_popup, Some(AppPopup::File { .. }));
        let response = ui.selectable_label(selected, "File");
        if response.clicked() {
            self.app_popup_had_focus = false;
            self.app_popup = if selected {
                None
            } else {
                Some(AppPopup::File {
                    anchor: response.rect.left_bottom(),
                })
            };
        }
    }

    fn show_edit_menu(&mut self, ui: &mut egui::Ui) {
        let selected = matches!(self.app_popup, Some(AppPopup::Edit { .. }));
        let response = ui.selectable_label(selected, "Edit");
        if response.clicked() {
            self.app_popup_had_focus = false;
            self.app_popup = if selected {
                None
            } else {
                Some(AppPopup::Edit {
                    anchor: response.rect.left_bottom(),
                })
            };
        }
    }

    fn editor_history_availability(&self, _context: &egui::Context) -> (bool, bool) {
        (
            self.document_kind.is_editable() && !self.editor_undo.is_empty(),
            self.document_kind.is_editable() && !self.editor_redo.is_empty(),
        )
    }

    fn undo_editor(&mut self, context: &egui::Context, redo: bool) {
        if !self.document_kind.is_editable() {
            return;
        }
        let current = self.editor_snapshot(context);
        let next = if redo {
            let next = self.editor_redo.pop();
            if next.is_some() {
                self.editor_undo.push(current);
            }
            next
        } else {
            let next = self.editor_undo.pop();
            if next.is_some() {
                self.editor_redo.push(current);
            }
            next
        };
        let Some(next) = next else {
            return;
        };
        let changed = next.source != self.source;
        self.source = next.source;
        self.store_editor_cursor(context, next.cursor);
        self.reset_editor_history = false;
        let editor_id = source_editor_id(context);
        context.memory_mut(|memory| memory.request_focus(editor_id));
        if changed {
            self.search.clear();
            self.mark_edited();
        }
    }

    fn editor_snapshot(&self, context: &egui::Context) -> EditorSnapshot {
        let cursor = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .and_then(|state| state.cursor.char_range())
            .unwrap_or_else(|| CCursorRange::one(CCursor::new(self.source.chars().count())));
        EditorSnapshot {
            source: self.source.clone(),
            cursor: clamp_cursor_range(cursor, self.source.chars().count()),
        }
    }

    fn store_editor_cursor(&self, context: &egui::Context, cursor: CCursorRange) {
        let mut state = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .unwrap_or_default();
        state.clear_undoer();
        state.cursor.set_char_range(Some(clamp_cursor_range(
            cursor,
            self.source.chars().count(),
        )));
        state.store(context, source_editor_id(context));
    }

    fn record_editor_undo_point(&mut self, context: &egui::Context) {
        let snapshot = self.editor_snapshot(context);
        self.push_editor_undo_snapshot(snapshot);
    }

    fn push_editor_undo_snapshot(&mut self, snapshot: EditorSnapshot) {
        if self
            .editor_undo
            .last()
            .is_none_or(|latest| latest.source != snapshot.source)
        {
            self.editor_undo.push(snapshot);
            const MAX_EDITOR_HISTORY: usize = 100;
            if self.editor_undo.len() > MAX_EDITOR_HISTORY {
                self.editor_undo.remove(0);
            }
        }
        self.editor_redo.clear();
    }

    fn selected_editor_chars(&self, context: &egui::Context) -> Option<Range<usize>> {
        let state = egui::text_edit::TextEditState::load(context, source_editor_id(context))?;
        let range = state.cursor.char_range()?.as_sorted_char_range();
        let len = self.source.chars().count();
        let range = range.start.0.min(len)..range.end.0.min(len);
        (range.start != range.end).then_some(range)
    }

    fn copy_editor_selection(&self, context: &egui::Context) {
        let Some(range) = self.selected_editor_chars(context) else {
            return;
        };
        let bytes = char_range_to_byte_range(&self.source, range);
        context.copy_text(self.source[bytes].to_owned());
    }

    fn cut_editor_selection(&mut self, context: &egui::Context) {
        if !self.document_kind.is_editable() {
            return;
        }
        let Some(range) = self.selected_editor_chars(context) else {
            return;
        };
        self.record_editor_undo_point(context);
        let bytes = char_range_to_byte_range(&self.source, range.clone());
        context.copy_text(self.source[bytes.clone()].to_owned());
        self.source.replace_range(bytes, "");
        let editor_id = source_editor_id(context);
        if let Some(mut state) = egui::text_edit::TextEditState::load(context, editor_id) {
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(range.start))));
            state.store(context, editor_id);
        }
        context.memory_mut(|memory| memory.request_focus(editor_id));
        self.search.clear();
        self.mark_edited();
    }

    fn request_editor_paste(&mut self, context: &egui::Context) {
        if self.document_kind.is_editable() {
            if self.document_kind.is_typst() && !self.view_mode.shows_code() {
                self.view_mode = ViewMode::Split;
            }
            let editor_id = source_editor_id(context);
            context.memory_mut(|memory| memory.request_focus(editor_id));
            context.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
    }

    fn select_all_editor(&self, context: &egui::Context) {
        let editor_id = source_editor_id(context);
        if let Some(mut state) = egui::text_edit::TextEditState::load(context, editor_id) {
            state.cursor.set_char_range(Some(CCursorRange::two(
                CCursor::new(0),
                CCursor::new(self.source.chars().count()),
            )));
            state.store(context, editor_id);
            context.memory_mut(|memory| memory.request_focus(editor_id));
        }
    }

    fn request_format_document(&mut self) {
        if !self.document_kind.is_typst() {
            return;
        }
        if !self.tinymist_lsp_ready {
            self.notice = Some(Notice {
                message: "Tinymist is still starting; try formatting again in a moment".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        let (Some(generation), Some(uri)) = (self.tinymist_generation, self.tinymist_uri.clone())
        else {
            self.notice = Some(Notice {
                message: "Tinymist is still starting; try formatting again in a moment".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        };
        if let Some(document) = &self.tinymist_unsaved_document
            && let Err(error) = document.update_backing_source(&self.source)
        {
            self.notice = Some(Notice {
                message: format!("Could not prepare the unsaved document for formatting: {error}"),
                kind: NoticeKind::Error,
            });
            return;
        }
        match self
            .tinymist
            .format_document(generation, uri, revision_as_i32(self.revision))
        {
            Ok(()) => {
                self.notice = Some(Notice {
                    message: "Formatting with Tinymist…".to_owned(),
                    kind: NoticeKind::Info,
                });
            }
            Err(error) => {
                self.notice = Some(Notice {
                    message: format!("Could not request formatting: {error}"),
                    kind: NoticeKind::Error,
                });
            }
        }
    }

    fn receive_formatted_document(
        &mut self,
        context: &egui::Context,
        generation: Generation,
        uri: &str,
        version: i32,
        edits: Option<Vec<LspTextEdit>>,
    ) {
        if self.tinymist_generation != Some(generation)
            || self.tinymist_uri.as_deref() != Some(uri)
            || version != revision_as_i32(self.revision)
        {
            return;
        }
        let Some(edits) = edits else {
            self.notice = Some(Notice {
                message: "Tinymist did not provide a formatter for this document".to_owned(),
                kind: NoticeKind::Error,
            });
            return;
        };
        if edits.is_empty() {
            self.notice = Some(Notice {
                message: "Document is already formatted".to_owned(),
                kind: NoticeKind::Success,
            });
            return;
        }
        let cursor = if self.reset_editor_history {
            CCursorRange::one(CCursor::new(0))
        } else {
            self.editor_snapshot(context).cursor
        };
        let applied = match apply_text_edits(
            &self.source,
            &edits,
            [cursor.primary.index.0, cursor.secondary.index.0],
        ) {
            Ok(applied) => applied,
            Err(error) => {
                self.notice = Some(Notice {
                    message: format!("Tinymist returned invalid formatting edits: {error}"),
                    kind: NoticeKind::Error,
                });
                return;
            }
        };
        let mapped_cursor = CCursorRange {
            primary: CCursor::new(applied.mapped_offsets[0]),
            secondary: CCursor::new(applied.mapped_offsets[1]),
            h_pos: cursor.h_pos,
        };
        let formatted = applied.text;
        if formatted == self.source {
            self.notice = Some(Notice {
                message: "Document is already formatted".to_owned(),
                kind: NoticeKind::Success,
            });
            return;
        }
        self.pending_editor_selection = None;
        self.push_editor_undo_snapshot(EditorSnapshot {
            source: self.source.clone(),
            cursor,
        });
        self.source = formatted;
        self.store_editor_cursor(context, mapped_cursor);
        self.reset_editor_history = false;
        self.search.clear();
        self.mark_edited();
        self.notice = Some(Notice {
            message: "Formatted with Tinymist".to_owned(),
            kind: NoticeKind::Success,
        });
    }

    fn begin_rename(&mut self, path: PathBuf) {
        let Some(name) = path.file_name() else {
            return;
        };
        self.rename_dialog = Some(RenameDialog {
            name: name.to_string_lossy().into_owned(),
            path,
            focus: true,
        });
        self.rename_overlay_had_focus = false;
        self.rename_overlay_suspended = false;
    }

    fn show_rename_dialog(&mut self, context: &egui::Context) {
        if self.rename_dialog.is_none() || self.app_modal.is_some() || self.rename_overlay_suspended
        {
            return;
        }
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let dialog_width = (window_rect.width() - METRICS.popup.rename_window_inset)
            .clamp(1.0, METRICS.popup.rename_max_width);
        let theme = context.theme();
        let style = context.style_of(theme);
        let native_theme = theme::native_theme(theme);
        let Some(dialog) = &mut self.rename_dialog else {
            return;
        };
        let mut submit = false;
        let mut cancel = false;
        let mut overlay_had_focus = self.rename_overlay_had_focus;
        let mut suspend_overlay = false;
        let captures = self.captures.clone();
        context.show_viewport_immediate(
            scoped_child_viewport_id(context, "tiptoptyp-rename-overlay"),
            theme::popup_viewport_builder("Rename")
                .with_position(window_rect.min)
                .with_inner_size(window_rect.size())
                .with_min_inner_size(window_rect.size())
                .with_max_inner_size(window_rect.size())
                .with_active(true),
            |ui, _class| {
                captures.begin_viewport(ui.ctx());
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                cancel |= ui.ctx().input(|input| {
                    if input.viewport().focused == Some(true) {
                        overlay_had_focus = true;
                    }
                    suspend_overlay |= overlay_had_focus && input.viewport().focused == Some(false);
                    input.viewport().close_requested() || input.key_pressed(egui::Key::Escape)
                });
                if suspend_overlay {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                egui::Area::new(viewport_scoped_id(ui.ctx(), "rename-document-dialog"))
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            ui.set_min_width(dialog_width);
                            ui.set_max_width(dialog_width);
                            ui.label(RichText::new("Rename").strong());
                            let response = ui.add_sized(
                                [dialog_width, METRICS.popup.rename_input_height],
                                egui::TextEdit::singleline(&mut dialog.name).id_salt("rename-name"),
                            );
                            if dialog.focus {
                                response.request_focus();
                                dialog.focus = false;
                            }
                            submit |= response.lost_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                submit |= ui.button("Rename").clicked();
                                cancel |= ui.button("Cancel").clicked();
                            });
                        });
                    });
                captures.end_glow_viewport(ui, "rename");
            },
        );
        self.rename_overlay_had_focus = overlay_had_focus;
        if suspend_overlay {
            self.rename_overlay_suspended = true;
        }
        if cancel {
            self.rename_dialog = None;
            self.rename_overlay_had_focus = false;
            self.rename_overlay_suspended = false;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
        } else if submit {
            let dialog = self.rename_dialog.take().expect("rename dialog exists");
            self.rename_overlay_had_focus = false;
            self.rename_overlay_suspended = false;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.commit_rename(dialog.path, dialog.name, context);
        }
    }

    fn show_workspace_chooser(&mut self, context: &egui::Context) {
        if !self.workspace_chooser_visible || self.app_modal.is_some() {
            return;
        }
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let style = context.style_of(context.theme());
        let native_theme = theme::native_theme(context.theme());
        let card_width = (window_rect.width() - METRICS.popup.modal_window_inset)
            .clamp(1.0, METRICS.popup.modal_max_width);
        let recent_workspaces = if self.snapshot_scene == Some(UiSnapshotScene::WorkspaceChooser) {
            vec![
                PathBuf::from("Recent/Annual report"),
                PathBuf::from("Recent/Research notes"),
            ]
        } else {
            self.settings.existing_recent_workspaces()
        };
        let mut selected = None;
        let mut choose_folder = false;
        let mut cancel = false;
        let mut dismiss = false;
        let captures = self.captures.clone();

        context.show_viewport_immediate(
            scoped_child_viewport_id(context, "tiptoptyp-workspace-chooser"),
            theme::popup_viewport_builder("Change workspace root")
                .with_position(window_rect.min)
                .with_inner_size(window_rect.size())
                .with_min_inner_size(window_rect.size())
                .with_max_inner_size(window_rect.size())
                .with_active(true),
            |ui, _class| {
                captures.begin_viewport(ui.ctx());
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                dismiss |= ui.ctx().input(|input| {
                    input.viewport().close_requested() || input.key_pressed(egui::Key::Escape)
                });
                egui::Area::new(viewport_scoped_id(ui.ctx(), "workspace-chooser-card"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            ui.set_width(card_width);
                            ui.label(RichText::new("Change workspace root").strong());
                            ui.label(
                                RichText::new(
                                    "Choose a recent workspace or select another folder.",
                                )
                                .color(ui.visuals().weak_text_color()),
                            );
                            if !recent_workspaces.is_empty() {
                                ui.add_space(theme::SPACE.small);
                                ui.label(RichText::new("Recent").small().strong());
                                egui::ScrollArea::vertical()
                                    .id_salt("recent-workspaces")
                                    .max_height(METRICS.popup.modal_message_max_height)
                                    .show(ui, |ui| {
                                        for path in &recent_workspaces {
                                            let path_text = path.display().to_string();
                                            let max_chars = approximate_char_capacity(
                                                card_width - theme::SPACE.content * 2.0,
                                                theme::TYPE.supporting,
                                            );
                                            let response = ui.add_sized(
                                                [ui.available_width(), METRICS.menu.row_height],
                                                egui::Button::new(tail_elide(
                                                    &path_text, max_chars,
                                                )),
                                            );
                                            native_hover_text(response.clone(), path_text);
                                            if response.clicked() {
                                                selected = Some(path.clone());
                                            }
                                        }
                                    });
                            }
                            ui.add_space(theme::SPACE.control);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                choose_folder |= ui.button("Choose Folder…").clicked();
                                cancel |= ui.button("Cancel").clicked();
                            });
                        });
                    });
                captures.end_glow_viewport(ui, "workspace");
            },
        );

        if let Some(path) = selected {
            self.workspace_chooser_visible = false;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.queue_open_path(path);
        } else if choose_folder {
            self.workspace_chooser_visible = false;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.open_folder_dialog();
        } else if cancel || dismiss {
            self.workspace_chooser_visible = false;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn show_app_modal_window(&mut self, context: &egui::Context) {
        if self.app_modal_suspended {
            return;
        }
        let Some(modal) = self.app_modal.clone() else {
            return;
        };
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let native_theme = theme::native_theme(theme);
        let card_width = (window_rect.width() - METRICS.popup.modal_window_inset)
            .clamp(1.0, METRICS.popup.modal_max_width);
        let message_height = (window_rect.height() - METRICS.popup.modal_height_inset).clamp(
            METRICS.popup.modal_message_min_height,
            METRICS.popup.modal_message_max_height,
        );
        let mut choice = None;
        let mut overlay_had_focus = self.app_modal_had_focus;
        let mut suspend_overlay = false;
        let captures = self.captures.clone();

        context.show_viewport_immediate(
            scoped_child_viewport_id(context, "tiptoptyp-modal-overlay"),
            theme::popup_viewport_builder("tiptoptyp")
                .with_position(window_rect.min)
                .with_inner_size(window_rect.size())
                .with_min_inner_size(window_rect.size())
                .with_max_inner_size(window_rect.size())
                .with_active(true),
            |ui, _class| {
                captures.begin_viewport(ui.ctx());
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                if ui.ctx().input(|input| {
                    if input.viewport().focused == Some(true) {
                        overlay_had_focus = true;
                    }
                    suspend_overlay |= overlay_had_focus && input.viewport().focused == Some(false);
                    input.viewport().close_requested() || input.key_pressed(egui::Key::Escape)
                }) {
                    choice = Some(AppModalChoice::Cancel);
                }
                if suspend_overlay {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                egui::Area::new(viewport_scoped_id(ui.ctx(), "app-modal-card"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            ui.set_max_width(card_width);
                            ui.set_min_width(card_width);
                            let (title, message, color) = match &modal {
                                AppModal::Alert {
                                    title,
                                    message,
                                    kind,
                                } => (
                                    title.as_str(),
                                    message.as_str(),
                                    notice_color(*kind, ui.visuals().dark_mode),
                                ),
                                AppModal::Unsaved { message, .. } => (
                                    "warning",
                                    message.as_str(),
                                    warning_color(ui.visuals().dark_mode),
                                ),
                                AppModal::Overwrite { message, .. } => (
                                    "warning",
                                    message.as_str(),
                                    warning_color(ui.visuals().dark_mode),
                                ),
                            };
                            ui.label(RichText::new(title).strong().color(color));
                            egui::ScrollArea::vertical()
                                .max_height(message_height)
                                .show(ui, |ui| {
                                    ui.label(message);
                                });
                            ui.add_space(theme::SPACE.control);
                            ui.with_layout(
                                Layout::right_to_left(Align::Center),
                                |ui| match &modal {
                                    AppModal::Alert { .. } => {
                                        if ui.button("OK").clicked() {
                                            choice = Some(AppModalChoice::Primary);
                                        }
                                    }
                                    AppModal::Unsaved { .. } => {
                                        if ui.button("Save").clicked() {
                                            choice = Some(AppModalChoice::Primary);
                                        }
                                        if ui.button("Discard").clicked() {
                                            choice = Some(AppModalChoice::Secondary);
                                        }
                                        if ui.button("Cancel").clicked() {
                                            choice = Some(AppModalChoice::Cancel);
                                        }
                                    }
                                    AppModal::Overwrite { .. } => {
                                        if ui.button("Overwrite").clicked() {
                                            choice = Some(AppModalChoice::Primary);
                                        }
                                        if ui.button("Cancel").clicked() {
                                            choice = Some(AppModalChoice::Cancel);
                                        }
                                    }
                                },
                            );
                        });
                    });
                captures.end_glow_viewport(ui, "modal");
            },
        );
        self.app_modal_had_focus = overlay_had_focus;
        if suspend_overlay {
            self.app_modal_suspended = true;
        }

        let Some(choice) = choice else {
            return;
        };
        self.app_modal = None;
        self.app_modal_had_focus = false;
        self.app_modal_suspended = false;
        context.send_viewport_cmd(egui::ViewportCommand::Focus);
        match (modal, choice) {
            (AppModal::Alert { .. }, _) => {}
            (AppModal::Unsaved { pending, .. }, AppModalChoice::Primary) => {
                self.pending_document_action = Some(PendingDocumentAction {
                    action: DeferredDocumentAction::SaveThen(Box::new(pending)),
                    document_epoch: self.document_epoch,
                    revision: self.revision,
                    allow_discard: true,
                    description: "saving the current document".to_owned(),
                });
            }
            (AppModal::Unsaved { mut pending, .. }, AppModalChoice::Secondary) => {
                self.post_save_action = None;
                pending.document_epoch = self.document_epoch;
                pending.revision = self.revision;
                pending.allow_discard = true;
                self.pending_document_action = Some(pending);
            }
            (AppModal::Unsaved { .. }, AppModalChoice::Cancel) => {
                self.post_save_action = None;
                self.schedule_autosave_if_needed();
            }
            (
                AppModal::Overwrite {
                    path,
                    document_epoch,
                    revision,
                    expected_disk_fingerprint,
                    observed_disk_fingerprint,
                    ..
                },
                AppModalChoice::Primary,
            ) => {
                self.pending_document_action = Some(PendingDocumentAction {
                    action: DeferredDocumentAction::ForceSave {
                        path,
                        document_epoch,
                        revision,
                        expected_disk_fingerprint,
                        observed_disk_fingerprint,
                    },
                    document_epoch: self.document_epoch,
                    revision: self.revision,
                    allow_discard: true,
                    description: "overwriting the current file".to_owned(),
                });
            }
            (AppModal::Overwrite { .. }, _) => {
                self.post_save_action = None;
                self.schedule_autosave_if_needed();
            }
        }
        context.request_repaint();
    }

    fn commit_rename(&mut self, old_path: PathBuf, name: String, context: &egui::Context) {
        let name = name.trim();
        let mut components = Path::new(name).components();
        let valid_name = matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none();
        if !valid_name {
            self.notice = Some(Notice {
                message: "Enter a filename without folders".to_owned(),
                kind: NoticeKind::Error,
            });
            self.begin_rename(old_path);
            return;
        }
        let Some(parent) = old_path.parent() else {
            return;
        };
        let new_path = parent.join(name);
        if old_path == new_path {
            return;
        }
        let renames_current_document = self
            .path
            .as_ref()
            .is_some_and(|current| same_path(current, &old_path));
        let renames_preview_document = self
            .designated_preview_path()
            .is_some_and(|preview| same_path(&preview, &old_path));
        let reload_binary_document = renames_current_document && self.document_kind.preview_only();
        let destination_is_same_entry = old_path
            .canonicalize()
            .ok()
            .zip(new_path.canonicalize().ok())
            .is_some_and(|(old, new)| old == new);
        if fs::symlink_metadata(&new_path).is_ok() && !destination_is_same_entry {
            self.notice = Some(Notice {
                message: format!("{} already exists", new_path.display()),
                kind: NoticeKind::Error,
            });
            self.begin_rename(old_path);
            return;
        }
        if let Err(error) = fs::rename(&old_path, &new_path) {
            self.show_file_error(format!("Could not rename {}: {error}", old_path.display()));
            return;
        }
        let new_path = new_path.canonicalize().unwrap_or(new_path);
        if renames_preview_document {
            let root = canonical_or_absolute(&self.workspace_root);
            if let (Some(key), Some(value)) = (root.to_str(), new_path.to_str()) {
                self.settings
                    .preview_files
                    .insert(key.to_owned(), value.to_owned());
                if let Some(settings) = &mut self.pending_settings {
                    settings
                        .preview_files
                        .insert(key.to_owned(), value.to_owned());
                }
            }
        }
        if reload_binary_document {
            // Reload from the renamed file so detection sees the real binary
            // bytes. Using `self.source` here would pass an empty buffer for
            // PDFs/images and could incorrectly turn them into editable text.
            self.load_path(new_path.clone());
        } else if renames_current_document {
            self.path = Some(new_path.clone());
            self.document_epoch = self.document_epoch.wrapping_add(1);
            self.document_kind = DocumentKind::detect(&new_path, self.source.as_bytes())
                .ok()
                .filter(|kind| kind.is_editable())
                .unwrap_or(DocumentKind::Text);
            self.remember_open_document(&new_path);
            self.reset_document_services();
            if self.document_kind.is_typst() {
                self.schedule_compile_now();
            } else {
                self.status = PreviewStatus::Ready(Duration::ZERO);
            }
        }
        self.refresh_workspace();
        if renames_preview_document && !renames_current_document {
            self.restart_tinymist();
            self.schedule_compile_now();
            self.rebuild_project_index(context);
        }
        self.notice = Some(Notice {
            message: format!("Renamed to {}", new_path.display()),
            kind: NoticeKind::Success,
        });
    }

    fn show_settings_window(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        if !self.settings_visible || self.app_modal.is_some() || self.rename_dialog.is_some() {
            return;
        }
        let active_theme = self
            .imported_theme
            .as_ref()
            .map_or(context.theme(), |theme| {
                if theme.dark_mode {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                }
            });
        let style = context.style_of(active_theme);
        let native_theme = theme::native_theme(active_theme);
        let mut close_requested = false;
        let captures = self.captures.clone();
        context.show_viewport_immediate(
            scoped_child_viewport_id(context, "tiptoptyp-settings"),
            egui::ViewportBuilder::default()
                .with_title("tiptoptyp Settings")
                .with_inner_size([
                    METRICS.chrome.settings_width,
                    METRICS.chrome.settings_height,
                ])
                .with_min_inner_size(METRICS.chrome.settings_min_size)
                .with_decorations(false)
                .with_fullsize_content_view(true)
                .with_title_shown(false)
                .with_titlebar_shown(false)
                .with_maximize_button(false)
                .with_maximized(false)
                .with_fullscreen(false),
            |ui, _class| {
                captures.begin_viewport(ui.ctx());
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                let settings_tooltip_id = settings_hover_tooltip_id(ui.ctx());
                ui.ctx()
                    .data_mut(|data| data.remove::<HoverTooltipOverlay>(settings_tooltip_id));
                close_requested |= ui.ctx().input(|input| input.viewport().close_requested());
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
                            theme::show_logo(ui);
                            ui.label(RichText::new("Settings").strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon_button(ui, UiIcon::Close, "Close Settings").clicked() {
                                    close_requested = true;
                                }
                            });
                        });
                    });
                egui::CentralPanel::default()
                    .frame(theme::settings_content_frame(ui.style()))
                    .show(ui, |ui| self.show_settings(ui, frame));
                if self.snapshot_scene == Some(UiSnapshotScene::SettingsTooltip) {
                    show_local_tooltip_card(
                        ui.ctx(),
                        Pos2::new(
                            theme::SPACE.content * 2.0,
                            METRICS.chrome.toolbar_height + theme::SPACE.content * 2.0,
                        ),
                        "Built-in and imported themes share the same semantic colors.",
                        1.0,
                    );
                } else if let Some(tooltip) = ui
                    .ctx()
                    .data(|data| data.get_temp::<HoverTooltipOverlay>(settings_tooltip_id))
                {
                    show_local_tooltip_card(
                        ui.ctx(),
                        tooltip.anchor,
                        &tooltip.detail,
                        tooltip.opacity,
                    );
                }
                captures.end_glow_viewport(ui, "settings");
            },
        );
        if close_requested {
            self.settings_visible = false;
        }
    }

    fn show_typst_overrides_window(&mut self, context: &egui::Context) {
        if !self.typst_overrides_visible
            || self.app_modal.is_some()
            || self.rename_dialog.is_some()
            || self.pending_tool_picker.is_some()
        {
            return;
        }

        let deterministic = self.snapshot_scene == Some(UiSnapshotScene::TypstOverridesWindow);
        let rendered_dark = self.typst_overrides_dark;
        let mut selected_dark = rendered_dark;
        let mut edited = if deterministic {
            AppSettings::default()
        } else {
            self.pending_settings
                .clone()
                .unwrap_or_else(|| self.settings.clone())
        };
        if deterministic {
            let overrides = edited.typst_overrides.for_dark_mut(rendered_dark);
            overrides.get_mut_or_default(TypstSyntaxRole::Function).bold = Some(true);
        }

        let request = theme_request_for_appearance(&edited, rendered_dark);
        let (preview_theme, fallback) = self
            .imported_theme
            .as_ref()
            .filter(|theme| theme.dark_mode == rendered_dark)
            .map_or_else(
                || load_active_theme_or_fallback(&request),
                |theme| (theme.clone(), None),
            );
        if deterministic {
            let accent = preview_theme.palette.accent;
            let overrides = edited.typst_overrides.for_dark_mut(rendered_dark);
            overrides
                .get_mut_or_default(TypstSyntaxRole::Keyword)
                .foreground = Some(accent);
            overrides
                .get_mut_or_default(TypstSyntaxRole::Raw)
                .background = Some(Rgba {
                a: METRICS.syntax.override_sample_background_alpha,
                ..accent
            });
        }
        let appearance = if preview_theme.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        let style = theme::style_for_semantic_palette(
            context.style_of(appearance).as_ref(),
            preview_theme.dark_mode,
            preview_theme.palette,
        );
        let native_theme = theme::native_theme(appearance);
        let syntax_palette = theme::syntax_palette_from_semantic(preview_theme.palette);
        let original_overrides = edited.typst_overrides.clone();
        let captures = self.captures.clone();
        let mut close_requested = false;

        context.show_viewport_immediate(
            scoped_child_viewport_id(context, "tiptoptyp-typst-overrides"),
            egui::ViewportBuilder::default()
                .with_title("tiptoptyp Typst Overrides")
                .with_inner_size([
                    METRICS.chrome.typst_overrides_width,
                    METRICS.chrome.typst_overrides_height,
                ])
                .with_min_inner_size(METRICS.chrome.typst_overrides_min_size)
                .with_decorations(false)
                .with_fullsize_content_view(true)
                .with_title_shown(false)
                .with_titlebar_shown(false)
                .with_maximize_button(false)
                .with_maximized(false)
                .with_fullscreen(false),
            |ui, _class| {
                captures.begin_viewport(ui.ctx());
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                let overrides_tooltip_id = typst_overrides_hover_tooltip_id(ui.ctx());
                ui.ctx()
                    .data_mut(|data| data.remove::<HoverTooltipOverlay>(overrides_tooltip_id));
                close_requested |= ui.ctx().input(|input| input.viewport().close_requested());
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
                    ui.id().with("typst-overrides-window-drag"),
                    Sense::drag(),
                );
                if drag.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                egui::Panel::top("typst-overrides-titlebar")
                    .exact_size(METRICS.chrome.toolbar_height)
                    .frame(theme::settings_title_frame(ui.style()))
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            theme::show_logo(ui);
                            ui.label(RichText::new("Typst overrides").strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon_button(ui, UiIcon::Close, "Close Typst overrides").clicked()
                                {
                                    close_requested = true;
                                }
                            });
                        });
                    });
                egui::CentralPanel::default()
                    .frame(theme::settings_content_frame(ui.style()))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            theme::apply_compact_control_spacing(ui);
                            ui.label(RichText::new("Appearance").strong());
                            ui.selectable_value(&mut selected_dark, false, "Light");
                            ui.selectable_value(&mut selected_dark, true, "Dark");
                            ui.separator();
                            if ui
                                .add_enabled(
                                    !edited.typst_overrides.for_dark(rendered_dark).is_empty(),
                                    egui::Button::new("Reset this appearance"),
                                )
                                .clicked()
                            {
                                edited.typst_overrides.for_dark_mut(rendered_dark).clear();
                            }
                            ui.label(
                                RichText::new("Unset fields inherit the selected theme")
                                    .small()
                                    .weak(),
                            );
                        });
                        if let Some(reason) = &fallback {
                            fallback_notice(ui, "THEME FALLBACK", reason);
                        }
                        ui.add_space(theme::SPACE.small);
                        egui::ScrollArea::both()
                            .id_salt("typst-overrides-scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                show_typst_override_editor(
                                    ui,
                                    edited.typst_overrides.for_dark_mut(rendered_dark),
                                    syntax_palette,
                                    &preview_theme.syntect_theme,
                                );
                            });
                    });
                if let Some(tooltip) = ui
                    .ctx()
                    .data(|data| data.get_temp::<HoverTooltipOverlay>(overrides_tooltip_id))
                {
                    show_local_tooltip_card(
                        ui.ctx(),
                        tooltip.anchor,
                        &tooltip.detail,
                        tooltip.opacity,
                    );
                }
                captures.end_glow_viewport(ui, "typst-overrides");
            },
        );

        self.typst_overrides_dark = selected_dark;
        if close_requested {
            self.typst_overrides_visible = false;
        }
        if !deterministic && edited.typst_overrides != original_overrides {
            self.queue_settings(edited, context);
        }
    }

    fn show_diagnostic_tooltip_window(&self, context: &egui::Context) {
        let native_tooltip_id = native_hover_tooltip_id(context);
        let (origin, anchor, detail, severity, opacity) = if self.snapshot_scene
            == Some(UiSnapshotScene::DiagnosticTooltip)
        {
            (
                Rect::from_min_size(
                    Pos2::new(420.0, METRICS.chrome.toolbar_height + 86.0),
                    Vec2::splat(1.0),
                ),
                Pos2::new(420.0, METRICS.chrome.toolbar_height + 86.0),
                "The character `#` is not valid in code\nHint: you are already in code mode\nHint: try removing the `#`".to_owned(),
                Some(DiagnosticSeverity::Error),
                1.0,
            )
        } else if self.snapshot_scene == Some(UiSnapshotScene::FunctionTooltip) {
            (
                Rect::from_min_size(
                    Pos2::new(420.0, METRICS.chrome.toolbar_height + 86.0),
                    Vec2::splat(1.0),
                ),
                Pos2::new(420.0, METRICS.chrome.toolbar_height + 86.0),
                "text(body, size: length = 1em, fill: color = black)\nDisplays content as text with the selected size and fill."
                    .to_owned(),
                None,
                1.0,
            )
        } else if let Some(tooltip) = self.diagnostic_tooltip.clone() {
            (
                tooltip.origin,
                tooltip.anchor,
                tooltip.detail,
                Some(tooltip.severity),
                tooltip.opacity,
            )
        } else if let Some(tooltip) =
            context.data(|data| data.get_temp::<HoverTooltipOverlay>(native_tooltip_id))
        {
            (
                tooltip.origin,
                tooltip.anchor,
                tooltip.detail,
                None,
                tooltip.opacity,
            )
        } else {
            return;
        };
        let root_ready = matches!(
            self.snapshot_scene,
            Some(UiSnapshotScene::DiagnosticTooltip | UiSnapshotScene::FunctionTooltip)
        ) || context.input(|input| {
            input.viewport().focused == Some(true) && input.viewport().visible() != Some(false)
        });
        if !root_ready
            || self.settings_visible
            || self.rename_dialog.is_some()
            || self.app_popup.is_some()
            || self.app_modal.is_some()
        {
            return;
        }
        show_native_tooltip_card(
            context,
            "diagnostic-tooltip-overlay",
            anchor,
            origin,
            &detail,
            severity,
            opacity,
            &self.captures,
        );
    }

    fn show_app_popup_window(&mut self, context: &egui::Context) {
        if self.app_modal.is_some() {
            return;
        }
        let Some(popup) = self.app_popup.clone() else {
            return;
        };
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            self.app_popup = None;
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let native_theme = theme::native_theme(theme);
        let (anchor, desired_size) = match &popup {
            AppPopup::File { anchor } => (*anchor, METRICS.menu.file_size),
            AppPopup::Edit { anchor } => (*anchor, METRICS.menu.edit_size),
            AppPopup::Workspace { anchor, .. } => (*anchor, METRICS.menu.workspace_size),
            AppPopup::Editor { anchor } => (*anchor, METRICS.menu.editor_size),
            AppPopup::StatusLog { anchor } => (*anchor, METRICS.menu.status_log_size),
        };
        let estimated_size = egui::vec2(
            desired_size
                .x
                .min((window_rect.width() - METRICS.popup.viewport_edge).max(1.0)),
            desired_size
                .y
                .min((window_rect.height() - METRICS.popup.viewport_edge).max(1.0)),
        );
        let menu_width = (estimated_size.x - METRICS.popup.menu_horizontal_chrome).max(1.0);
        let menu_height = (estimated_size.y - METRICS.popup.menu_vertical_chrome).max(1.0);
        let anchor = clamp_popup_anchor(anchor, estimated_size, window_rect.size());
        let (can_undo, can_redo) = self.editor_history_availability(context);
        let has_selection = self.selected_editor_chars(context).is_some();
        let can_format = self.document_kind.is_typst();
        let mut close = false;
        let mut action = None;
        let mut had_focus = self.app_popup_had_focus;
        let captures = self.captures.clone();

        context.show_viewport_immediate(
            scoped_child_viewport_id(context, "tiptoptyp-popup-overlay"),
            theme::popup_viewport_builder("tiptoptyp menu")
                .with_position(window_rect.min + anchor.to_vec2())
                .with_inner_size(estimated_size)
                .with_min_inner_size(estimated_size)
                .with_max_inner_size(estimated_size)
                .with_active(true),
            |ui, _class| {
                captures.begin_viewport(ui.ctx());
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                close |= ui.ctx().input(|input| {
                    if input.viewport().focused == Some(true) {
                        had_focus = true;
                    }
                    input.viewport().close_requested()
                        || input.key_pressed(egui::Key::Escape)
                        || (had_focus && input.viewport().focused == Some(false))
                });

                egui::Area::new(viewport_scoped_id(ui.ctx(), "app-popup-card"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(Pos2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::menu_card_frame(&style).show(ui, |ui| {
                            ui.set_min_width(menu_width);
                            ui.set_max_width(menu_width);
                            egui::ScrollArea::vertical()
                                .max_height(menu_height)
                                .show(ui, |ui| match &popup {
                                    AppPopup::File { .. } => {
                                        show_file_popup_ui(ui, &mut action);
                                    }
                                    AppPopup::Edit { .. } => {
                                        show_edit_popup_ui(
                                            ui,
                                            can_undo,
                                            can_redo,
                                            can_format,
                                            &mut action,
                                        );
                                    }
                                    AppPopup::Workspace { path, is_file, .. } => {
                                        show_workspace_popup_ui(ui, path, *is_file, &mut action);
                                    }
                                    AppPopup::Editor { .. } => {
                                        let mut editor_action = None;
                                        show_editor_context_menu_ui(
                                            ui,
                                            can_undo,
                                            can_redo,
                                            has_selection,
                                            can_format,
                                            &mut editor_action,
                                        );
                                        if let Some(editor_action) = editor_action {
                                            action = Some(AppPopupAction::Editor(editor_action));
                                        }
                                    }
                                    AppPopup::StatusLog { .. } => {
                                        show_status_log_popup_ui(ui, &self.status_log);
                                    }
                                });
                        });
                    });
                captures.end_glow_viewport(ui, "popup");
            },
        );
        self.app_popup_had_focus = had_focus;

        let action_selected = action.is_some();
        if close || action_selected {
            self.app_popup = None;
            self.app_popup_had_focus = false;
        }
        // A popup losing focus usually means the user activated another app.
        // Only an in-app menu selection should return keyboard focus to the
        // editor; reclaiming it on blur makes tiptoptyp steal activation.
        if action_selected {
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if let Some(action) = action {
            self.pending_app_popup_action = Some(action);
            context.request_repaint();
        }
    }

    fn execute_pending_app_popup_action(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        let Some(action) = self.pending_app_popup_action.take() else {
            return;
        };
        match action {
            AppPopupAction::File(action) => self.execute_file_menu_action(action, frame),
            AppPopupAction::Editor(action) => self.execute_editor_menu_action(action, context),
            AppPopupAction::Find(replace) => self.open_find(replace),
            AppPopupAction::Workspace(action) => match action {
                WorkspaceMenuAction::Open(path) => {
                    if self
                        .path
                        .as_ref()
                        .is_none_or(|current| !same_path(current, &path))
                    {
                        self.request_document_replacement(
                            DeferredDocumentAction::LoadPath(path),
                            "opening another project file",
                        );
                    }
                }
                WorkspaceMenuAction::UseForPreview(path) => {
                    self.use_file_for_preview(path, context)
                }
                WorkspaceMenuAction::Rename(path) => self.begin_rename(path),
                WorkspaceMenuAction::CopyPath(path) => {
                    context.copy_text(path.display().to_string());
                }
                WorkspaceMenuAction::Reveal(path) => {
                    if let Err(error) = reveal_in_file_manager(&path) {
                        self.notice = Some(Notice {
                            message: error,
                            kind: NoticeKind::Error,
                        });
                    }
                }
            },
        }
    }

    fn show_settings(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        ui.set_min_width(ui.available_width());

        let deterministic_settings = matches!(
            self.snapshot_scene,
            Some(
                UiSnapshotScene::SettingsWindow
                    | UiSnapshotScene::SettingsThemePicker
                    | UiSnapshotScene::SettingsDarkThemePicker
                    | UiSnapshotScene::SettingsTooltip
            )
        );
        let mut edited = if deterministic_settings {
            AppSettings::default()
        } else {
            self.pending_settings
                .clone()
                .unwrap_or_else(|| self.settings.clone())
        };
        egui::ScrollArea::vertical()
            .id_salt("settings-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading("Appearance");
                let system_theme = ui.ctx().system_theme();
                let effective_theme = ui.ctx().theme();
                let theme_overridden = self.theme_override.is_some();
                let theme_picker_enabled = !theme_overridden
                    || matches!(
                        self.snapshot_scene,
                        Some(
                            UiSnapshotScene::SettingsThemePicker
                                | UiSnapshotScene::SettingsDarkThemePicker
                        )
                    );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.add_sized(
                        [
                            METRICS.settings.appearance_label_width,
                            METRICS.icon.button_size.y,
                        ],
                        egui::Label::new(RichText::new("Appearance").strong()),
                    );
                    ui.add_enabled_ui(theme_picker_enabled, |ui| {
                        for appearance in InterfaceTheme::ALL {
                            ui.selectable_value(
                                &mut edited.interface_theme,
                                appearance,
                                appearance.label(),
                            );
                        }
                    });
                    if theme_overridden {
                        ui.label(RichText::new("QA override").small().weak());
                    }
                    ui.separator();
                    settings_inline_value(ui, "Active", theme_label(Some(effective_theme)));
                });

                for (appearance, label, picker_id) in [
                    (egui::Theme::Light, "Light theme", "light-color-theme"),
                    (egui::Theme::Dark, "Dark theme", "dark-color-theme"),
                ] {
                    let selected_name = color_theme_choice_label(edited.color_theme(appearance));
                    let imported_path = match edited.color_theme(appearance) {
                        ColorThemeChoice::Sublime(path) => Some(path.clone()),
                        ColorThemeChoice::Builtin(_) => None,
                    };
                    ui.horizontal_wrapped(|ui| {
                        theme::apply_compact_control_spacing(ui);
                        ui.add_sized(
                            [
                                METRICS.settings.appearance_label_width,
                                METRICS.icon.button_size.y,
                            ],
                            egui::Label::new(RichText::new(label).strong()),
                        );
                        ui.add_enabled_ui(theme_picker_enabled, |ui| {
                            let open_picker = matches!(
                                (appearance, self.snapshot_scene),
                                (
                                    egui::Theme::Light,
                                    Some(UiSnapshotScene::SettingsThemePicker)
                                ) | (
                                    egui::Theme::Dark,
                                    Some(UiSnapshotScene::SettingsDarkThemePicker)
                                )
                            );
                            if open_picker {
                                // `ComboBox` stores an already-hashed `IdSalt`, so use
                                // the same representation rather than hashing the raw
                                // string along a different ID path.
                                let id =
                                    ui.make_persistent_id(egui::IdSalt::new(picker_id));
                                egui::Popup::open_id(ui.ctx(), id.with("popup"));
                            }
                            let choice = edited.color_theme_mut(appearance);
                            let picker = egui::ComboBox::from_id_salt(picker_id)
                                .width(220.0)
                                .height(METRICS.settings.theme_picker_max_height)
                                .selected_text(selected_name)
                                .show_ui(ui, |ui| {
                                    for builtin in builtin_themes::for_mode(
                                        appearance == egui::Theme::Dark,
                                    ) {
                                        let selected = matches!(
                                            choice,
                                            ColorThemeChoice::Builtin(id) if id == builtin.id
                                        );
                                        if ui
                                            .selectable_label(selected, builtin.name)
                                            .clicked()
                                        {
                                            *choice = ColorThemeChoice::builtin(builtin.id);
                                        }
                                    }
                                })
                                .response;
                            if let Some(path) = &imported_path {
                                settings_hover_text(picker, path.clone());
                            }
                            if ui.button("Import…").clicked() {
                                self.choose_tool_binary(
                                    ToolPickerTarget::SublimeTheme {
                                        dark_mode: appearance == egui::Theme::Dark,
                                    },
                                    frame,
                                    ui.ctx(),
                                );
                            }
                        });
                    });
                }

                let (mut displayed_invert, mut displayed_hue_shift) = self
                    .theme_override
                    .as_ref()
                    .map_or((edited.theme_invert, edited.theme_hue_shift_degrees), |profile| {
                        (profile.invert, profile.hue_shift_degrees)
                    });
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new("Transform").strong());
                    ui.add_enabled_ui(!theme_overridden, |ui| {
                        ui.checkbox(&mut displayed_invert, "Invert");
                        ui.separator();
                        ui.label("Hue");
                        ui.add_sized(
                            [190.0, METRICS.icon.button_size.y],
                            egui::Slider::new(&mut displayed_hue_shift, -180..=180).suffix("°"),
                        );
                        if ui
                            .add_enabled(
                                displayed_invert || displayed_hue_shift != 0,
                                egui::Button::new("Reset"),
                            )
                            .clicked()
                        {
                            displayed_invert = false;
                            displayed_hue_shift = 0;
                        }
                    });
                    ui.label(
                        RichText::new("both themes · invert, then hue")
                            .small()
                            .weak(),
                    );
                });
                if !theme_overridden {
                    edited.theme_invert = displayed_invert;
                    edited.theme_hue_shift_degrees = displayed_hue_shift;
                }
                if !deterministic_settings
                    && edited.interface_theme == InterfaceTheme::System
                    && system_theme.is_none()
                {
                    fallback_notice(
                        ui,
                        "THEME FALLBACK",
                        "System appearance is unavailable; using the configured dark theme",
                    );
                }

                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Page").strong());
                    for theme in DocumentTheme::ALL {
                        ui.selectable_value(&mut edited.document_theme, theme, theme.label());
                    }
                    ui.separator();
                    settings_inline_value(
                        ui,
                        "Effective",
                        theme_label(Some(edited.document_theme.resolve(effective_theme))),
                    );
                });

                ui.add_space(theme::SPACE.small);
                ui.separator();
                ui.heading("Editor");
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut edited.line_wrap, "Wrap lines");
                    ui.checkbox(&mut edited.line_numbers, "Line numbers");
                    if ui.button("Typst overrides…").clicked() {
                        self.typst_overrides_dark = effective_theme == egui::Theme::Dark;
                        self.typst_overrides_visible = true;
                    }
                    ui.checkbox(&mut edited.auto_save, "Auto-save");
                    ui.add_enabled_ui(edited.auto_save, |ui| {
                        ui.label("Auto-save delay");
                        ui.add(
                            egui::Slider::new(&mut edited.auto_save_delay_ms, 250..=5_000)
                                .suffix(" ms")
                                .logarithmic(true),
                        );
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new("Preview jump").strong());
                    let trigger = egui::ComboBox::from_id_salt("source-preview-trigger")
                        .width(METRICS.settings.source_preview_trigger_width)
                        .selected_text(edited.source_preview_trigger.label())
                        .show_ui(ui, |ui| {
                            for trigger in SourcePreviewTrigger::ALL {
                                settings_hover_text(
                                    ui.selectable_value(
                                        &mut edited.source_preview_trigger,
                                        trigger,
                                        trigger.label(),
                                    ),
                                    trigger.description(),
                                );
                            }
                        })
                        .response;
                    settings_hover_text(trigger, edited.source_preview_trigger.description());
                    ui.separator();
                    ui.label(RichText::new("Hovers").strong());
                    ui.label("wait");
                    ui.add(
                        egui::DragValue::new(&mut edited.hover_delay_ms)
                            .range(0..=2_000)
                            .speed(10)
                            .suffix(" ms"),
                    );
                    ui.label("fade");
                    ui.add(
                        egui::DragValue::new(&mut edited.hover_fade_ms)
                            .range(0..=500)
                            .speed(5)
                            .suffix(" ms"),
                    );
                });

                ui.add_space(theme::SPACE.small);
                ui.separator();
                ui.heading("Tool binaries");
                ui.label(
                    RichText::new(
                        "Packaged builds use pinned sidecars. A custom path overrides one tool without changing the other.",
                    )
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
                let staged_typst = if edited.typst == self.settings.typst {
                    self.typst_tool.clone()
                } else {
                    resolve_tool(ToolKind::Typst, &edited.typst)
                };
                if tool_preference_editor(
                    ui,
                    "Typst compiler",
                    &mut edited.typst,
                    &staged_typst,
                    deterministic_settings,
                ) {
                    self.choose_tool_binary(ToolPickerTarget::Typst, frame, ui.ctx());
                }
                if !deterministic_settings
                    && let Some(reason) = &staged_typst.fallback_reason
                {
                    fallback_notice(ui, "BINARY FALLBACK ACTIVE", reason);
                }
                ui.add_space(METRICS.settings.tool_gap);
                let staged_tinymist = if edited.tinymist == self.settings.tinymist {
                    self.tinymist_tool.clone()
                } else {
                    resolve_tool(ToolKind::Tinymist, &edited.tinymist)
                };
                if tool_preference_editor(
                    ui,
                    "Tinymist language server",
                    &mut edited.tinymist,
                    &staged_tinymist,
                    deterministic_settings,
                ) {
                    self.choose_tool_binary(ToolPickerTarget::Tinymist, frame, ui.ctx());
                }
                if !deterministic_settings
                    && let Some(reason) = &staged_tinymist.fallback_reason
                {
                    fallback_notice(ui, "BINARY FALLBACK ACTIVE", reason);
                }
                if ui.button("Refresh binary status").clicked() {
                    self.tool_refresh_requested = true;
                    ui.ctx().request_repaint();
                }

                ui.add_space(theme::SPACE.small);
                ui.separator();
                ui.heading("Preview backend");
                ui.horizontal_wrapped(|ui| {
                    for preference in PreviewPreference::ALL {
                        ui.selectable_value(
                            &mut edited.preview_preference,
                            preference,
                            preference.label(),
                        );
                    }
                    ui.separator();
                    settings_inline_value(
                        ui,
                        "Effective",
                        if deterministic_settings {
                            "Interactive"
                        } else {
                            self.preview_backend_label()
                        },
                    );
                });
                if !deterministic_settings
                    && let Some(reason) = self.preview_fallback_reason()
                {
                    fallback_notice(ui, "PREVIEW FALLBACK ACTIVE", &reason);
                }
                if !deterministic_settings
                    && self.settings.preview_preference == PreviewPreference::Interactive
                    && !self.interactive_preview_active()
                    && ui.button("Retry Tinymist").clicked()
                {
                    self.restart_tinymist();
                }

                ui.add_space(theme::SPACE.small);
                ui.separator();
                ui.heading("Toolchain status");
                ui.horizontal_wrapped(|ui| {
                    let syntax_color = success_color(ui.visuals().dark_mode);
                    if deterministic_settings {
                        show_status_chip(ui, "Typst", "Bundled", "Packaged compiler", syntax_color);
                        show_status_chip(
                            ui,
                            "Tinymist",
                            "Bundled",
                            "Packaged language server",
                            syntax_color,
                        );
                        for name in ["LSP", "Vector", "Watcher", "PDF"] {
                            show_status_chip(ui, name, "Ready", "Ready", syntax_color);
                        }
                    } else {
                        show_tool_status_chip(ui, "Typst", &self.typst_tool);
                        show_tool_status_chip(ui, "Tinymist", &self.tinymist_tool);
                        show_service_status_chip(ui, "LSP", &self.tinymist_state);
                        show_service_status_chip(ui, "Vector", &self.webview_state);
                        show_service_status_chip(ui, "Watcher", &self.compiler_service_state());
                        show_service_status_chip(ui, "PDF", &self.rasterizer_service_state());
                    }
                    show_status_chip(
                        ui,
                        "Syntax",
                        "Ready",
                        "typst-syntax (official parser)",
                        syntax_color,
                    );
                });
                settings_value_row(
                    ui,
                    "Project root",
                    &if deterministic_settings {
                        "Theme gallery workspace".to_owned()
                    } else {
                        self.project_root().display().to_string()
                    },
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("UI screenshots").strong());
                    if ui.button("Main").clicked() {
                        self.captures.queue("main", "main");
                    }
                    if ui.button("Settings").clicked() {
                        self.captures.queue("settings", "settings");
                    }
                    if ui.button("Both").clicked() {
                        self.captures.queue("main", "main");
                        self.captures.queue("settings", "settings");
                    }
                    settings_hover_text(
                        ui.label(RichText::new("Cmd+Shift+F12").small().weak()),
                        format!(
                            "App-window-only PNGs are saved under {}",
                            self.captures.output_directory().display()
                        ),
                    );
                });
            });

        if !deterministic_settings {
            self.queue_settings(edited, ui.ctx());
        }
    }

    fn compiler_service_state(&self) -> ServiceState {
        if !self.document_kind.is_typst() {
            return ServiceState::Disabled("The selected file is not compiled as Typst".to_owned());
        }
        match self.status {
            PreviewStatus::Waiting => {
                ServiceState::Starting("Build queued for persistent `typst watch`".to_owned())
            }
            PreviewStatus::Compiling => {
                ServiceState::Starting("Persistent `typst watch` is compiling".to_owned())
            }
            PreviewStatus::Ready(elapsed) => ServiceState::Ready(format!(
                "Canonical PDF ready in {:.0} ms",
                elapsed.as_secs_f64() * 1000.0
            )),
            PreviewStatus::Error => {
                let detail = self
                    .raw_diagnostics
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("The current document did not build")
                    .to_owned();
                if detail.contains("Could not start Typst")
                    || detail.contains("watch` stopped unexpectedly")
                {
                    ServiceState::Failed(detail)
                } else {
                    ServiceState::Degraded(detail)
                }
            }
        }
    }

    fn rasterizer_service_state(&self) -> ServiceState {
        if self.document_kind == DocumentKind::Text {
            return ServiceState::Disabled("Text files do not need a preview renderer".to_owned());
        }
        if self.document_kind == DocumentKind::Image && !self.pages.is_empty() {
            return ServiceState::Ready("The selected image decoded successfully".to_owned());
        }
        if !self.pages.is_empty() && self.compiled_revision == Some(self.revision) {
            return ServiceState::Ready(format!(
                "Poppler rendered {} page(s) at {} DPI",
                self.pages.len(),
                crate::compiler::PREVIEW_DPI
            ));
        }
        if !self.pages.is_empty() {
            return ServiceState::Degraded(format!(
                "Showing {} page(s) from the last successful build",
                self.pages.len()
            ));
        }
        match self.status {
            PreviewStatus::Error => {
                ServiceState::Failed("No rasterized pages are available".to_owned())
            }
            PreviewStatus::Waiting | PreviewStatus::Compiling | PreviewStatus::Ready(_) => {
                ServiceState::Starting("Waiting for the watched PDF to render".to_owned())
            }
        }
    }

    fn show_workspace(&mut self, ui: &mut egui::Ui) {
        let root = if self.snapshot_scene.is_some() {
            "Theme gallery workspace".to_owned()
        } else {
            self.workspace
                .as_ref()
                .map(|workspace| workspace.root().display().to_string())
                .unwrap_or_else(|| self.project_root().display().to_string())
        };
        let generation = self.workspace.as_ref().map(WorkspaceTree::generation);
        theme::panel_header(ui, "workspace-header", |ui| {
            let action_width = METRICS.explorer.header_refresh_width;
            let actions_width = action_width + ui.spacing().item_spacing.x;
            let path_width = (ui.available_width() - actions_width).max(1.0);
            let max_chars = approximate_char_capacity(path_width, theme::TYPE.supporting);
            let response = ui.add_sized(
                [path_width, METRICS.explorer.header_row_height],
                egui::Label::new(
                    RichText::new(tail_elide(&root, max_chars))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                )
                .truncate()
                .sense(Sense::click()),
            );
            let hover = generation.map_or_else(
                || format!("{root}\nDouble-click to change workspace root"),
                |generation| {
                    format!(
                        "{root}\nFilesystem snapshot generation {generation}\nDouble-click to change workspace root"
                    )
                },
            );
            if native_hover_text(response, hover).double_clicked() {
                self.open_workspace_chooser();
            }
            if icon_button(ui, UiIcon::Refresh, "Refresh filesystem").clicked() {
                self.refresh_workspace();
            }
        });

        // A tree row can be much wider than the pane. Keep the body width in a
        // clipped child UI so it becomes scrollable content instead of feeding
        // back into `PanelState` and growing the resizable explorer each frame.
        let mut content_ui = clipped_panel_content_ui(ui, "workspace-clipped-content");
        let ui = &mut content_ui;
        ui.spacing_mut().item_spacing.y = METRICS.explorer.section_gap;

        let snapshot = self.workspace.as_ref().map(WorkspaceTree::snapshot);
        let project_root = snapshot.map_or(self.workspace_root.as_path(), |snapshot| {
            snapshot.root.as_path()
        });
        let preview_path = self.designated_preview_path();
        let project_index = &self.project_index;
        let active = snapshot.as_ref().and_then(|snapshot| {
            self.path.as_ref().and_then(|path| {
                path.strip_prefix(&snapshot.root)
                    .ok()
                    .and_then(|relative| snapshot.find(relative))
                    .map(|node| node.path.clone())
            })
        });
        let mut open_path = None;
        let mut index_target = None;
        let mut popup_request = None;
        let section_body_height = explorer_section_body_height(ui);
        explorer_section(
            ui,
            "workspace-files",
            "Files",
            true,
            section_body_height,
            |ui| {
                if let Some(snapshot) = &snapshot {
                    let tree_id = ui.id().with((
                        "workspace-tree",
                        snapshot.root.clone(),
                        self.workspace.as_ref().map_or(0, WorkspaceTree::generation),
                    ));
                    let mut tree_state = TreeViewState::load(ui, tree_id).unwrap_or_default();
                    if let Some(active) = &active {
                        // Keep the document shown in the editor selected so the
                        // entire explorer row gets the same kind of tint as the
                        // editor's active line.
                        tree_state.set_one_selected(active.clone());
                    }
                    let tree = TreeView::new(tree_id)
                        .allow_multi_selection(false)
                        .fallback_context_menu(|ui, selected: &Vec<PathBuf>| {
                            let Some(path) = selected.first().cloned() else {
                                ui.close();
                                return;
                            };
                            let is_file = path
                                .strip_prefix(&snapshot.root)
                                .ok()
                                .and_then(|relative| snapshot.find(relative))
                                .is_some_and(WorkspaceNode::is_file);
                            let anchor = ui
                                .ctx()
                                .pointer_latest_pos()
                                .unwrap_or_else(|| ui.min_rect().left_top());
                            popup_request = Some(AppPopup::Workspace {
                                anchor,
                                path,
                                is_file,
                            });
                            ui.close();
                        });
                    let (_, actions) = ui
                        .scope(|ui| {
                            theme::apply_active_row_selection(ui);
                            tree.show_state(ui, &mut tree_state, |builder| {
                                add_workspace_nodes(
                                    builder,
                                    &snapshot.nodes,
                                    active.as_deref(),
                                    preview_path.as_deref(),
                                );
                            })
                        })
                        .inner;
                    tree_state.store(ui, tree_id);
                    for action in actions {
                        if let TreeAction::Activate(activate) = action {
                            open_path = activate.selected.into_iter().find(|path| {
                                path.strip_prefix(&snapshot.root)
                                    .ok()
                                    .and_then(|relative| snapshot.find(relative))
                                    .is_some_and(WorkspaceNode::is_file)
                            });
                        }
                    }
                } else {
                    ui.label(RichText::new("No project folder").weak());
                }
                if let Some(error) = &self.workspace_error {
                    ui.colored_label(error_color(ui.visuals().dark_mode), error);
                }
            },
        );

        show_project_index_sections(
            ui,
            project_root,
            project_index,
            section_body_height,
            &mut index_target,
        );

        if let Some(path) = open_path
            && self
                .path
                .as_ref()
                .is_none_or(|current| !same_path(current, &path))
        {
            self.request_document_replacement(
                DeferredDocumentAction::LoadPath(path),
                "opening another project file",
            );
        }
        if let Some((path, line)) = index_target {
            if self
                .path
                .as_ref()
                .is_some_and(|current| same_path(current, &path))
            {
                self.apply_file_link_location(None, Some((line, 1)));
            } else {
                self.request_document_replacement(
                    DeferredDocumentAction::FollowFileLink {
                        path,
                        page: None,
                        source_position: Some((line, 1)),
                    },
                    "opening a project index entry",
                );
            }
        }
        if let Some(popup) = popup_request {
            self.app_popup_had_focus = false;
            self.app_popup = Some(popup);
        }
    }

    fn show_find_bar(&mut self, ui: &mut egui::Ui) {
        let mut find_next = false;
        let mut find_previous = false;
        let mut replace_one = false;
        let mut replace_all = false;
        let match_count = find_all(&self.source, &self.find_query).len();

        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.find_query)
                    .id_salt("find-query")
                    .hint_text("Find")
                    .desired_width(METRICS.editor.find_field_width),
            );
            if self.focus_find {
                response.request_focus();
                self.focus_find = false;
            }
            if response.changed() {
                self.search.clear();
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                find_next = true;
            }
            ui.label(
                RichText::new(format!("{match_count} matches"))
                    .small()
                    .weak(),
            );
            find_previous |= icon_button(ui, UiIcon::Up, "Previous match · Shift+Enter").clicked();
            find_next |= icon_button(ui, UiIcon::Down, "Next match · Enter").clicked();
            if icon_button(ui, UiIcon::Close, "Close · Esc").clicked() {
                self.find_visible = false;
                self.replace_visible = false;
                self.search.clear();
            }
        });

        if self.replace_visible {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.replacement)
                        .hint_text("Replace")
                        .desired_width(METRICS.editor.find_field_width),
                );
                replace_one |= ui.button("Replace").clicked();
                replace_all |= ui.button("All").clicked();
            });
        }

        if find_previous {
            self.pending_editor_selection = self
                .search
                .previous(&self.source, &self.find_query)
                .map(|matched| matched.char_range.clone());
        }
        if find_next {
            self.pending_editor_selection = self
                .search
                .next(&self.source, &self.find_query)
                .map(|matched| matched.char_range.clone());
        }
        if replace_one {
            let before = self.source.clone();
            let snapshot = self.editor_snapshot(ui.ctx());
            let replaced =
                self.search
                    .replace_one(&mut self.source, &self.find_query, &self.replacement);
            if replaced {
                self.pending_editor_selection = self
                    .search
                    .selected()
                    .map(|matched| matched.char_range.clone());
                if self.source != before {
                    self.push_editor_undo_snapshot(snapshot);
                    self.mark_edited();
                }
            }
        }
        if replace_all {
            let before = self.source.clone();
            let snapshot = self.editor_snapshot(ui.ctx());
            let count =
                self.search
                    .replace_all(&mut self.source, &self.find_query, &self.replacement);
            if count > 0 && self.source != before {
                self.push_editor_undo_snapshot(snapshot);
                self.notice = Some(Notice {
                    message: format!("Replaced {count} matches"),
                    kind: NoticeKind::Success,
                });
                self.mark_edited();
            }
        }
    }

    fn show_editor(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(ui.available_width());
        if self.reset_editor_history {
            let mut state =
                egui::text_edit::TextEditState::load(ui.ctx(), source_editor_id(ui.ctx()))
                    .unwrap_or_default();
            state.clear_undoer();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            state.store(ui.ctx(), source_editor_id(ui.ctx()));
            self.reset_editor_history = false;
        }
        if self.find_visible {
            self.show_find_bar(ui);
            ui.separator();
        }

        let line_diagnostics = self.line_diagnostics();
        let available_width = ui.available_width();
        let line_count = logical_line_count(&self.source);
        let longest_line = self
            .source
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        let longest_diagnostic = line_diagnostics
            .iter()
            .map(|diagnostic| diagnostic.summary.chars().count())
            .max()
            .unwrap_or(0);
        let unwrapped_editor_width = available_width
            .max(
                (longest_line as f32 * METRICS.editor.source_character_width)
                    + (longest_diagnostic as f32 * METRICS.editor.diagnostic_character_width)
                    + METRICS.editor.unwrapped_width_padding,
            )
            .max(METRICS.editor.unwrapped_minimum_width);
        let line_wrap = self.settings.line_wrap;
        let line_numbers = self.settings.line_numbers;
        let gutter_width = line_number_gutter_width(line_count, line_numbers);
        let dark_mode = ui.visuals().dark_mode;
        let document_kind = self.document_kind;
        let highlight_path = self.path.clone();
        let source_preview_trigger = self.settings.source_preview_trigger;
        let preview_jump_enabled = document_kind.is_typst() && self.interactive_preview_active();
        let snapshot_before_edit = self.editor_snapshot(ui.ctx());
        let highlighter = &mut self.highlighter;
        let generic_highlighter = &mut self.generic_highlighter;
        let pending_selection = self.pending_editor_selection.take();
        let attention = self.editor_attention.and_then(|attention| {
            let elapsed = Instant::now().saturating_duration_since(attention.started);
            let progress = editor_attention_progress(elapsed);
            (progress < 1.0).then_some((attention.char_index, progress))
        });
        if attention.is_some() {
            ui.ctx()
                .request_repaint_after(METRICS.motion.animation_frame);
        } else {
            self.editor_attention = None;
        }
        let mut changed = false;
        let mut preview_jump_char = None;
        let mut hovered_semantic_token = None;
        let mut popup_request = None;
        let editor_base_slot = ui.painter().add(egui::Shape::Noop);
        let editor_margin = egui::Margin {
            left: gutter_width,
            right: theme::SPACE.small as i8,
            top: theme::SPACE.tight as i8,
            bottom: theme::SPACE.tight as i8,
        };

        let scroll_output = egui::ScrollArea::new([!line_wrap, true])
            .id_salt("source-editor-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let editor_width = if line_wrap {
                    ui.available_width()
                        .max(METRICS.editor.wrapped_minimum_width)
                } else {
                    unwrapped_editor_width
                };
                let current_line_slot = ui.painter().add(egui::Shape::Noop);
                let background_slots = line_diagnostics
                    .iter()
                    .map(|_| ui.painter().add(egui::Shape::Noop))
                    .collect::<Vec<_>>();
                let mut layouter =
                    |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                        let mut job = if document_kind.is_typst() {
                            highlighter.highlight(buffer.as_str(), dark_mode, generic_highlighter)
                        } else {
                            generic_highlighter.highlight(
                                buffer.as_str(),
                                highlight_path.as_deref(),
                                dark_mode,
                            )
                        };
                        job.wrap.max_width = if line_wrap { wrap_width } else { f32::INFINITY };
                        ui.fonts_mut(|fonts| fonts.layout_job(job))
                    };
                let editor = egui::TextEdit::multiline(&mut self.source)
                    .id(source_editor_id(ui.ctx()))
                    .code_editor()
                    .desired_width(editor_width)
                    .min_size(Vec2::new(editor_width, 0.0))
                    // The surface is painted across the complete viewport
                    // below. Keep TextEdit's own frame empty so its intrinsic
                    // document height cannot leave an internal border behind.
                    .frame(egui::Frame::new().inner_margin(editor_margin))
                    .layouter(&mut layouter);
                let mut output = editor.show(ui);
                changed = output.response.changed();
                let mut current_char = output
                    .state
                    .cursor
                    .char_range()
                    .map(|range| range.primary.index.0);
                let line_rows = logical_line_row_ranges(&output.galley.rows);

                if let Some(range) = pending_selection {
                    let len = self.source.chars().count();
                    let range = range.start.min(len)..range.end.min(len);
                    let cursor_range =
                        CCursorRange::two(CCursor::new(range.start), CCursor::new(range.end));
                    output.state.cursor.set_char_range(Some(cursor_range));
                    current_char = Some(range.end);
                    output.state.clone().store(ui.ctx(), output.response.id);
                    output.response.request_focus();
                    let cursor_rect = output
                        .galley
                        .pos_from_cursor(CCursor::new(range.start))
                        .translate(output.galley_pos.to_vec2());
                    ui.scroll_to_rect(cursor_rect, Some(Align::Center));
                }

                paint_editor_line_backgrounds(
                    ui,
                    &output,
                    &self.source,
                    current_char,
                    attention,
                    &line_rows,
                    current_line_slot,
                );
                self.diagnostic_tooltip = paint_line_diagnostics(
                    ui,
                    &output,
                    &line_diagnostics,
                    &line_rows,
                    background_slots,
                );
                if line_numbers {
                    paint_line_numbers(ui, &output, &line_rows);
                }

                if document_kind.is_typst()
                    && output.response.hovered()
                    && let Some(pointer) = ui.ctx().pointer_hover_pos()
                {
                    let char_index = output
                        .galley
                        .cursor_from_pos(pointer - output.galley_pos)
                        .index
                        .0;
                    if let Some(range) = typst_hover_token_range(&self.source, char_index) {
                        let start = output
                            .galley
                            .pos_from_cursor(CCursor::new(range.start))
                            .translate(output.galley_pos.to_vec2());
                        let end = output
                            .galley
                            .pos_from_cursor(CCursor::new(range.end))
                            .translate(output.galley_pos.to_vec2());
                        let token_rect = Rect::from_min_max(
                            start.left_top(),
                            Pos2::new(end.left().max(start.left() + 1.0), start.bottom()),
                        );
                        if token_rect.expand(theme::SPACE.tight).contains(pointer) {
                            hovered_semantic_token = Some((range, token_rect));
                        }
                    }
                }

                if output.response.secondary_clicked() {
                    let anchor = ui
                        .ctx()
                        .pointer_latest_pos()
                        .unwrap_or_else(|| output.response.rect.center());
                    popup_request = Some(AppPopup::Editor { anchor });
                }

                let jump_gesture = match source_preview_trigger {
                    SourcePreviewTrigger::DoubleClick => output.response.double_clicked(),
                    SourcePreviewTrigger::ModifierClick => {
                        output.response.clicked() && ui.input(|input| input.modifiers.command)
                    }
                    SourcePreviewTrigger::Disabled => false,
                };
                if preview_jump_enabled
                    && jump_gesture
                    && let Some(pointer) = output.response.interact_pointer_pos()
                {
                    preview_jump_char = Some(
                        output
                            .galley
                            .cursor_from_pos(pointer - output.galley_pos)
                            .index
                            .0,
                    );
                }

                let visuals = *ui.style().interact(&output.response);
                let border = if output.response.has_focus() {
                    ui.visuals().selection.stroke
                } else {
                    visuals.bg_stroke
                };
                (output.response.rect, visuals.corner_radius, border)
            });

        let (editor_rect, corner_radius, border) = scroll_output.inner;
        let surface_rect = editor_surface_rect(editor_rect, scroll_output.inner_rect);
        ui.painter().set(
            editor_base_slot,
            egui::Shape::rect_filled(
                surface_rect,
                corner_radius,
                ui.visuals().text_edit_bg_color(),
            ),
        );
        ui.painter()
            .rect_stroke(surface_rect, corner_radius, border, StrokeKind::Inside);

        if self.diagnostic_tooltip.is_none() {
            self.update_editor_hover(ui, hovered_semantic_token);
        } else {
            self.editor_hover = None;
        }

        let filler_top = editor_rect.bottom().clamp(
            scroll_output.inner_rect.top(),
            scroll_output.inner_rect.bottom(),
        );
        if filler_top < scroll_output.inner_rect.bottom() {
            let filler_rect = Rect::from_min_max(
                Pos2::new(scroll_output.inner_rect.left(), filler_top),
                scroll_output.inner_rect.right_bottom(),
            );
            let filler = ui.interact(
                filler_rect,
                source_editor_id(ui.ctx()).with("viewport-filler"),
                Sense::click(),
            );
            if filler.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
            if filler.clicked() {
                let editor_id = source_editor_id(ui.ctx());
                let mut state =
                    egui::text_edit::TextEditState::load(ui.ctx(), editor_id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(
                        self.source.chars().count(),
                    ))));
                state.store(ui.ctx(), editor_id);
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(editor_id));
            }
        }

        if let Some(popup) = popup_request {
            self.app_popup_had_focus = false;
            self.app_popup = Some(popup);
        }

        if changed {
            self.push_editor_undo_snapshot(snapshot_before_edit);
            self.search.clear();
            self.mark_edited();
        }
        if let Some(char_index) = preview_jump_char {
            self.jump_source_to_preview(char_index);
        }
    }

    fn update_editor_hover(&mut self, ui: &mut egui::Ui, hovered: Option<(Range<usize>, Rect)>) {
        let Some((range, rect)) = hovered else {
            self.editor_hover = None;
            return;
        };
        let (Some(generation), Some(uri)) = (self.tinymist_generation, self.tinymist_uri.clone())
        else {
            self.editor_hover = None;
            return;
        };
        let version = revision_as_i32(self.revision);
        let changed = self.editor_hover.as_ref().is_none_or(|hover| {
            hover.range != range || hover.uri != uri || hover.version != version
        });
        if changed {
            let request_token = self.next_editor_hover_token;
            self.next_editor_hover_token = self.next_editor_hover_token.wrapping_add(1).max(1);
            self.editor_hover = Some(EditorHoverState {
                range: range.clone(),
                request_token,
                uri: uri.clone(),
                version,
                requested: false,
                detail: None,
            });
        }

        let response = ui.interact(
            rect.expand(theme::SPACE.tight),
            source_editor_id(ui.ctx()).with(("semantic-hover", range.start, range.end)),
            Sense::hover(),
        );
        let opacity = hover_opacity(
            &response,
            native_hover_tooltip_id(ui.ctx()).with("semantic-hover-timing"),
        );

        let should_request = self.tinymist_lsp_ready
            && self.tinymist_current_open
            && self
                .editor_hover
                .as_ref()
                .is_some_and(|hover| !hover.requested);
        if should_request {
            let position = lsp_position_at_char(&self.source, range.start);
            if let Some(hover) = &mut self.editor_hover {
                hover.requested = true;
                let _ = self.tinymist.hover_document(
                    generation,
                    uri,
                    version,
                    position,
                    hover.request_token,
                );
            }
        }

        if let (Some(opacity), Some(detail)) = (
            opacity,
            self.editor_hover
                .as_ref()
                .and_then(|hover| hover.detail.as_ref()),
        ) {
            let tooltip_id = native_hover_tooltip_id(ui.ctx());
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    tooltip_id,
                    HoverTooltipOverlay {
                        origin: rect.expand(theme::SPACE.tight),
                        anchor: rect.left_bottom() + egui::vec2(0.0, METRICS.editor.tooltip_gap),
                        detail: detail.clone(),
                        opacity,
                    },
                );
            });
        }
    }

    fn diagnostic_targets_current_document(&self, diagnostic: &Diagnostic) -> bool {
        match &diagnostic.source {
            DiagnosticSource::Main => self.current_is_preview_document(),
            DiagnosticSource::File(path) => {
                self.path
                    .as_ref()
                    .is_some_and(|current| same_path(current, path))
                    || (self.path.is_none() && same_path(&self.tinymist_document_path(), path))
            }
            DiagnosticSource::Global => false,
        }
    }

    fn diagnostic_target_path(&self, diagnostic: &Diagnostic) -> Option<PathBuf> {
        match &diagnostic.source {
            DiagnosticSource::Main => Some(self.preview_document_path()),
            DiagnosticSource::File(path) if path.is_absolute() => Some(path.clone()),
            DiagnosticSource::File(path) => {
                let preview_dir = self
                    .preview_document_path()
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.project_root());
                let from_preview = preview_dir.join(path);
                if from_preview.exists() {
                    Some(from_preview)
                } else {
                    Some(self.project_root().join(path))
                }
            }
            DiagnosticSource::Global => None,
        }
    }

    fn jump_to_diagnostic(&mut self, diagnostic: Diagnostic) {
        let Some(location) = diagnostic.location else {
            return;
        };
        if self.diagnostic_targets_current_document(&diagnostic) {
            self.apply_file_link_location(None, Some((location.line, location.column)));
            return;
        }
        let Some(path) = self.diagnostic_target_path(&diagnostic) else {
            return;
        };
        self.request_document_replacement(
            DeferredDocumentAction::FollowFileLink {
                path,
                page: None,
                source_position: Some((location.line, location.column)),
            },
            "opening a diagnostic location",
        );
    }

    fn line_diagnostics(&self) -> Vec<LineDiagnostic> {
        let mut by_line: BTreeMap<usize, Vec<&Diagnostic>> = BTreeMap::new();
        for diagnostic in self.diagnostics.iter().chain(&self.tinymist_diagnostics) {
            if self.diagnostic_targets_current_document(diagnostic)
                && let Some(line) = diagnostic.line()
            {
                let entries = by_line.entry(line).or_default();
                if let Some(existing) = entries.iter_mut().find(|existing| {
                    existing.message == diagnostic.message
                        && existing.severity == diagnostic.severity
                }) {
                    if diagnostic.details.len() > existing.details.len() {
                        *existing = diagnostic;
                    }
                } else {
                    entries.push(diagnostic);
                }
            }
        }
        by_line
            .into_iter()
            .map(|(line, diagnostics)| {
                let severity = diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.severity)
                    .min_by_key(|severity| severity_rank(*severity))
                    .unwrap_or(DiagnosticSeverity::Unknown);
                let summary = diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let detail = diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.full_message())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                LineDiagnostic {
                    line,
                    severity,
                    summary,
                    detail,
                }
            })
            .collect()
    }

    fn show_preview(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // The interactive viewer needs no toolbar, so its content aligns
        // exactly with the code panel. Raster-only page controls get one fixed
        // row and never change height with build status.
        if !self.interactive_preview_active() && !self.interactive_preview_transitioning() {
            theme::panel_header(ui, "preview-header", |ui| {
                self.show_preview_header_controls(ui);
            });
        }

        // WKWebView is a separate native layer and cannot be read by egui's
        // app-window framebuffer capture. Use the matching rasterised page for
        // this one QA frame; no desktop capture API is involved.
        if self.captures.has_pending_for("main") {
            self.hide_webview();
            self.show_native_preview(ui);
            return;
        }

        if self.should_attempt_interactive_preview() {
            let rect = ui.available_rect_before_wrap();
            if self.update_webview(ui.ctx(), frame, rect, ui.visuals().panel_fill, true) {
                let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                ui.painter().rect_filled(rect, 0.0, preview_background(ui));
            } else if self.interactive_preview_transitioning() {
                self.hide_webview();
                show_preview_transition(ui);
            } else {
                self.hide_webview();
                self.show_native_preview(ui);
            }
        } else if self.interactive_preview_transitioning() {
            self.hide_webview();
            show_preview_transition(ui);
        } else {
            self.hide_webview();
            self.show_native_preview(ui);
        }
    }

    fn show_preview_header_controls(&mut self, ui: &mut egui::Ui) {
        let header_width = ui.available_width();
        if !self.interactive_preview_active() {
            let page_count = self.pages.len();
            if header_width >= METRICS.preview.header_pages_min_width {
                if icon_button_enabled(ui, self.visible_page > 0, UiIcon::Previous, "Previous page")
                    .clicked()
                {
                    self.requested_page = Some(self.visible_page - 1);
                }
                ui.label(if page_count == 0 {
                    "–/–".to_owned()
                } else {
                    format!("{}/{page_count}", self.visible_page + 1)
                });
                if icon_button_enabled(
                    ui,
                    self.visible_page + 1 < page_count,
                    UiIcon::Next,
                    "Next page",
                )
                .clicked()
                {
                    self.requested_page = Some(self.visible_page + 1);
                }
            }
            if header_width >= METRICS.preview.header_zoom_min_width {
                ui.separator();
                if icon_button(ui, UiIcon::ZoomOut, "Zoom out").clicked() {
                    self.requested_zoom = Some(
                        (self.zoom / METRICS.preview.zoom_step)
                            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                    );
                    self.fit_width = false;
                }
                if icon_button(ui, UiIcon::ZoomIn, "Zoom in").clicked() {
                    self.requested_zoom = Some(
                        (self.zoom * METRICS.preview.zoom_step)
                            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                    );
                    self.fit_width = false;
                }
                if header_width >= METRICS.preview.header_percent_min_width {
                    ui.label(format!("{:.0}%", self.zoom * 100.0));
                }
                if icon_button(
                    ui,
                    UiIcon::FitWidth,
                    if self.fit_width {
                        "Fit page width (on)"
                    } else {
                        "Fit page width"
                    },
                )
                .clicked()
                {
                    self.fit_width = !self.fit_width;
                }
            }
        }
    }

    fn show_native_preview(&mut self, ui: &mut egui::Ui) {
        let viewport_rect = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(viewport_rect, 0.0, preview_background(ui));
        if self.pages.is_empty() {
            ui.centered_and_justified(|ui| match self.status {
                PreviewStatus::Error => {
                    ui.label(RichText::new("Fix the diagnostics to render a preview").weak());
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Building preview…");
                    });
                }
            });
            return;
        }

        let widest_page = self
            .pages
            .iter()
            .map(|page| page.size[0] as f32 * PDF_POINTS_PER_PREVIEW_PIXEL)
            .fold(1.0_f32, f32::max);
        if self.fit_width {
            self.zoom = ((viewport_rect.width() - PAGE_MARGIN * 2.0) / widest_page)
                .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM);
        }

        let scroll_id = ui.make_persistent_id("pdf-preview-scroll");
        let pointer = ui.ctx().input(|input| input.pointer.latest_pos());
        let pinch = ui.ctx().input(|input| input.zoom_delta());
        let pinch_active = pointer.is_some_and(|pointer| viewport_rect.contains(pointer))
            && (pinch - 1.0).abs() > 0.001;
        let old_zoom = self.zoom;
        let new_zoom = if pinch_active {
            self.fit_width = false;
            (self.zoom * pinch).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM)
        } else {
            self.requested_zoom.take().unwrap_or(self.zoom)
        };
        if (new_zoom - old_zoom).abs() > f32::EPSILON {
            let anchor = pointer
                .filter(|pointer| viewport_rect.contains(*pointer))
                .unwrap_or_else(|| viewport_rect.center());
            let mut state = egui::scroll_area::State::load(ui.ctx(), scroll_id).unwrap_or_default();
            state.offset =
                zoom_anchored_offset(state.offset, anchor - viewport_rect.min, old_zoom, new_zoom);
            state.store(ui.ctx(), scroll_id);
            self.zoom = new_zoom;
            self.fit_width = false;
        }

        let geometries = page_stack_geometry(self.pages.iter().map(|page| page.size), self.zoom);
        let content_width = geometries
            .iter()
            .map(|page| page.size.x)
            .fold(viewport_rect.width(), f32::max)
            + PAGE_MARGIN * 2.0;
        let content_height = stack_height(&geometries);
        let requested_page = self.requested_page.take();
        let mut clicked_link = None;

        let output = egui::ScrollArea::both()
            .id_salt("pdf-preview-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_size(Vec2::new(content_width, content_height));
                let page_theme = theme::preview_palette(self.preview_dark);
                for (page, geometry) in self.pages.iter().zip(&geometries) {
                    let left = ((content_width - geometry.size.x) * 0.5).max(PAGE_MARGIN);
                    let rect = Rect::from_min_size(
                        Pos2::new(
                            ui.min_rect().left() + left,
                            ui.min_rect().top() + geometry.top,
                        ),
                        geometry.size,
                    );
                    let shadow_rect = rect
                        .translate(Vec2::new(0.0, METRICS.preview.shadow_offset_y))
                        .expand(METRICS.preview.shadow_expand);
                    ui.painter().rect_filled(
                        shadow_rect,
                        METRICS.preview.shadow_radius,
                        page_theme.shadow,
                    );
                    ui.painter().rect_filled(
                        rect,
                        METRICS.preview.page_radius,
                        page_theme.page_fill,
                    );
                    ui.painter().rect_stroke(
                        rect,
                        METRICS.preview.page_radius,
                        Stroke::new(METRICS.preview.page_border_width, page_theme.border),
                        StrokeKind::Outside,
                    );
                    ui.put(
                        rect,
                        egui::Image::new(&page.texture)
                            .fit_to_exact_size(geometry.size)
                            .alt_text(format!("PDF page {}", geometry.index + 1)),
                    );
                    for (link_index, link) in page.links.iter().enumerate() {
                        let [left, top, right, bottom] = link.rect;
                        let link_rect = Rect::from_min_max(
                            Pos2::new(
                                rect.left() + left * rect.width(),
                                rect.top() + top * rect.height(),
                            ),
                            Pos2::new(
                                rect.left() + right * rect.width(),
                                rect.top() + bottom * rect.height(),
                            ),
                        )
                        .expand(theme::SPACE.tight)
                        .intersect(rect);
                        if !link_rect.is_positive() {
                            continue;
                        }
                        let response = ui.interact(
                            link_rect,
                            ui.id().with(("pdf-link", geometry.index, link_index)),
                            Sense::click(),
                        );
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response.clicked() {
                            clicked_link = Some(link.target.clone());
                        }
                        native_hover_text(response, &link.target);
                    }
                    if requested_page == Some(geometry.index) {
                        ui.scroll_to_rect(rect, Some(Align::Min));
                    }
                }
            });
        self.visible_page = visible_page(
            &geometries,
            output.state.offset.y,
            output.inner_rect.height(),
        );
        if let Some(target) = clicked_link {
            self.follow_preview_link(&target);
        }
    }

    fn show_problems(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new("PROBLEMS").small().strong());
            ui.label(
                RichText::new(format!(
                    "{} diagnostics",
                    self.diagnostics.len() + self.tinymist_diagnostics.len()
                ))
                .small()
                .color(ui.visuals().weak_text_color()),
            );
        });
        ui.separator();
        let diagnostic_count = self.diagnostics.len() + self.tinymist_diagnostics.len();
        let mut jump_target = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if diagnostic_count == 0 {
                    ui.label(RichText::new("No compiler diagnostics").weak());
                }
                for (index, diagnostic) in self
                    .diagnostics
                    .iter()
                    .chain(&self.tinymist_diagnostics)
                    .enumerate()
                {
                    let color = diagnostic_color(diagnostic.severity, ui.visuals().dark_mode);
                    let background_slot = ui.painter().add(egui::Shape::Noop);
                    let row = ui
                        .scope(|ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(diagnostic.severity.label())
                                        .strong()
                                        .color(color),
                                );
                                if let Some(location) = diagnostic.location {
                                    ui.label(
                                        RichText::new(format!(
                                            "{}:{}",
                                            location.line, location.column
                                        ))
                                        .monospace()
                                        .color(ui.visuals().weak_text_color()),
                                    );
                                }
                                ui.label(&diagnostic.message);
                            });
                            for detail in &diagnostic.details {
                                ui.horizontal(|ui| {
                                    ui.add_space(METRICS.problems.detail_indent);
                                    ui.label(RichText::new(detail).small().monospace().weak());
                                });
                            }
                        })
                        .response
                        .interact(Sense::click());
                    let row = if self.snapshot_scene == Some(UiSnapshotScene::ProblemsPanel)
                        && index == 0
                    {
                        row.highlight()
                    } else {
                        row
                    };
                    if row.hovered() || row.highlighted() {
                        ui.painter().set(
                            background_slot,
                            egui::Shape::rect_filled(
                                row.rect,
                                theme::RADIUS.row as f32,
                                ui.visuals()
                                    .selection
                                    .bg_fill
                                    .gamma_multiply(METRICS.problems.hover_opacity),
                            ),
                        );
                    }
                    if diagnostic.location.is_some() {
                        native_hover_text(row.clone(), "Double-click to open this location");
                        if row.double_clicked() {
                            jump_target = Some(diagnostic.clone());
                        }
                    }
                }
                if !self.raw_diagnostics.is_empty() {
                    ui.collapsing("Raw CLI stdout", |ui| {
                        ui.label(RichText::new(&self.raw_diagnostics).monospace());
                    });
                }
            });
        if let Some(diagnostic) = jump_target {
            self.jump_to_diagnostic(diagnostic);
        }
    }

    fn record_status_transition(&mut self) {
        if self.recorded_status == Some(self.status) {
            return;
        }
        let kind = match self.status {
            PreviewStatus::Ready(_) => NoticeKind::Success,
            PreviewStatus::Error => NoticeKind::Error,
            PreviewStatus::Waiting | PreviewStatus::Compiling => NoticeKind::Info,
        };
        self.status_log.push_front(StatusLogEntry {
            detail: self.status_detail(),
            kind,
        });
        self.status_log.truncate(10);
        self.recorded_status = Some(self.status);
    }

    fn document_extension_label(&self) -> Option<String> {
        if self.document_kind.is_typst() {
            return None;
        }
        self.path
            .as_ref()
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
            .or_else(|| {
                Some(
                    match self.document_kind {
                        DocumentKind::Text => ".txt",
                        DocumentKind::Pdf => ".pdf",
                        DocumentKind::Image => ".img",
                        DocumentKind::Typst => return None,
                    }
                    .to_owned(),
                )
            })
    }

    fn cursor_coordinates(&self, context: &egui::Context) -> Option<(usize, usize)> {
        if !self.document_kind.is_editable() {
            return None;
        }
        let char_index = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .and_then(|state| state.cursor.char_range())
            .map_or(0, |range| range.primary.index.0)
            .min(self.source.chars().count());
        Some(line_column_at_char(&self.source, char_index))
    }

    fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        let mut fallbacks = if self.snapshot_scene.is_some() {
            Vec::new()
        } else {
            self.non_preview_fallback_details(ui.ctx().system_theme())
        };
        if self.snapshot_scene.is_none()
            && let Some(reason) = self.preview_fallback_reason()
        {
            fallbacks.insert(0, format!("Preview: {reason}"));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if self.document_kind.is_editable() {
                // This is intentionally the first right-to-left item: its
                // position is pinned to the window edge regardless of status
                // or notice length.
                ui.label(
                    RichText::new(format!(
                        "{} lines · {} chars",
                        logical_line_count(&self.source),
                        self.source.chars().count()
                    ))
                    .small(),
                );
            }
            if let Some((line, column)) = self.cursor_coordinates(ui.ctx()) {
                ui.separator();
                native_hover_text(
                    ui.label(
                        RichText::new(format!("Ln {line}, Col {column}"))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    ),
                    "Cursor position",
                );
            }

            let dark_mode = ui.visuals().dark_mode;
            let (icon, color, timing) = match self.status {
                PreviewStatus::Waiting => (UiIcon::Waiting, neutral_color(dark_mode), None),
                PreviewStatus::Compiling => (UiIcon::Refresh, info_color(dark_mode), None),
                PreviewStatus::Ready(elapsed) => (
                    UiIcon::Check,
                    success_color(dark_mode),
                    self.document_kind
                        .is_typst()
                        .then(|| format!("{:.0} ms", elapsed.as_secs_f64() * 1000.0)),
                ),
                PreviewStatus::Error => (UiIcon::Warning, error_color(dark_mode), None),
            };
            let status_detail = self.status_detail();
            let status_response = ui
                .horizontal(|ui| {
                    if matches!(self.status, PreviewStatus::Ready(_))
                        && let Some(extension) = self.document_extension_label()
                    {
                        ui.label(RichText::new(extension).small().strong().color(color));
                    } else {
                        static_icon(ui, icon, color);
                    }
                    if let Some(timing) = timing {
                        ui.label(RichText::new(timing).small().strong().color(color));
                    }
                })
                .response
                .interact(Sense::click());
            native_hover_text(
                status_response.clone(),
                format!("{status_detail}\nDouble-click for recent status"),
            );
            if status_response.double_clicked() {
                self.app_popup_had_focus = false;
                self.app_popup = Some(AppPopup::StatusLog {
                    anchor: status_response.rect.left_top(),
                });
            }
            if !fallbacks.is_empty() {
                ui.separator();
                let count = fallbacks.len();
                let color = warning_color(ui.visuals().dark_mode);
                let fallback_detail = fallbacks.join("\n");
                native_hover_text(
                    ui.label(
                        RichText::new(count.to_string())
                            .small()
                            .strong()
                            .color(color),
                    ),
                    &fallback_detail,
                );
                native_hover_text(static_icon(ui, UiIcon::Warning, color), &fallback_detail);
            }
            if let Some(notice) = &self.notice {
                ui.separator();
                let color = match notice.kind {
                    NoticeKind::Info => info_color(ui.visuals().dark_mode),
                    NoticeKind::Success => success_color(ui.visuals().dark_mode),
                    NoticeKind::Error => error_color(ui.visuals().dark_mode),
                };
                native_hover_text(
                    ui.add(
                        egui::Label::new(RichText::new(&notice.message).small().color(color))
                            .truncate()
                            .halign(Align::RIGHT),
                    ),
                    &notice.message,
                );
            }
        });
    }

    fn status_detail(&self) -> String {
        if !self.document_kind.is_typst() {
            return match (self.document_kind, self.status) {
                (DocumentKind::Text, _) => "Text file ready".to_owned(),
                (DocumentKind::Image, PreviewStatus::Compiling) => "Decoding image".to_owned(),
                (DocumentKind::Image, PreviewStatus::Ready(_)) => "Image ready".to_owned(),
                (DocumentKind::Pdf, PreviewStatus::Compiling) => "Rendering PDF".to_owned(),
                (DocumentKind::Pdf, PreviewStatus::Ready(_)) => "PDF ready".to_owned(),
                (_, PreviewStatus::Error) => self
                    .notice
                    .as_ref()
                    .map(|notice| notice.message.clone())
                    .unwrap_or_else(|| "Could not open file".to_owned()),
                _ => "Opening file".to_owned(),
            };
        }
        match self.status {
            PreviewStatus::Waiting => "PDF build queued".to_owned(),
            PreviewStatus::Compiling => "Compiling PDF".to_owned(),
            PreviewStatus::Ready(elapsed) => {
                format!("PDF ready in {:.0} ms", elapsed.as_secs_f64() * 1000.0)
            }
            PreviewStatus::Error => self
                .raw_diagnostics
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("PDF build failed")
                .to_owned(),
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn update_webview(
        &mut self,
        context: &egui::Context,
        frame: &mut eframe::Frame,
        rect: Rect,
        background: Color32,
        visible: bool,
    ) -> bool {
        use wry::dpi::{LogicalPosition, LogicalSize};

        let Some(url) = self.tinymist_url.clone() else {
            self.hide_webview();
            return false;
        };
        let bounds = wry::Rect {
            position: LogicalPosition::new(rect.left() as f64, rect.top() as f64).into(),
            size: LogicalSize::new(rect.width().max(1.0) as f64, rect.height().max(1.0) as f64)
                .into(),
        };
        if self.webview.is_none() {
            let focused = context.input(|input| input.viewport().focused);
            let may_create =
                may_create_window_webview(self.window_host, cfg!(target_os = "macos"), focused);
            if !may_create {
                // Wry currently activates NSApplication while constructing a
                // WKWebView, including child web views. Defer creation until
                // the user next activates this window so background service
                // changes can never pull focus from another application.
                self.webview_state = ServiceState::Starting(
                    "Interactive preview will resume when the window is active".to_owned(),
                );
                return false;
            }
            let navigation_base = url.clone();
            let navigation_sender = self.web_link_sender.clone();
            let navigation_context = context.clone();
            let navigation_root = self.project_root();
            let navigation_dir = self.current_directory();
            let popup_base = url.clone();
            let popup_sender = self.web_link_sender.clone();
            let popup_context = context.clone();
            let popup_root = navigation_root.clone();
            let popup_dir = navigation_dir.clone();
            let rgba = (
                background.r(),
                background.g(),
                background.b(),
                background.a(),
            );
            let builder = wry::WebViewBuilder::new()
                .with_url(&url)
                .with_bounds(bounds)
                .with_background_color(rgba)
                .with_background_throttling(wry::BackgroundThrottlingPolicy::Disabled)
                // Enable WKWebView/WebView2's platform zoom gestures and
                // standard zoom shortcuts instead of emulating trackpads in
                // the egui layer.
                .with_hotkeys_zoom(true)
                .with_navigation_handler(move |candidate| {
                    if candidate == "about:blank" {
                        true
                    } else if same_web_origin(&navigation_base, &candidate) {
                        if let Some(target) = preview_document_file_url(
                            &candidate,
                            &navigation_root,
                            navigation_dir.as_deref(),
                        ) {
                            if navigation_sender.send(target).is_ok() {
                                navigation_context.request_repaint();
                            }
                            false
                        } else {
                            true
                        }
                    } else {
                        if navigation_sender.send(candidate).is_ok() {
                            navigation_context.request_repaint();
                        }
                        false
                    }
                })
                .with_new_window_req_handler(move |candidate, _features| {
                    let target = if same_web_origin(&popup_base, &candidate) {
                        preview_document_file_url(&candidate, &popup_root, popup_dir.as_deref())
                    } else {
                        Some(candidate)
                    };
                    if let Some(target) = target
                        && popup_sender.send(target).is_ok()
                    {
                        popup_context.request_repaint();
                    }
                    wry::NewWindowResponse::Deny
                });
            let built = if self.window_host.is_root() {
                let Some(window) = frame.winit_window() else {
                    self.webview_state = ServiceState::Degraded(
                        "The native window handle is temporarily unavailable".to_owned(),
                    );
                    return false;
                };
                builder.build_as_child(window.as_ref())
            } else {
                let Some(window) = self.native_window_parent.as_ref() else {
                    self.webview_state = ServiceState::Starting(
                        "Waiting for this document window's native handle".to_owned(),
                    );
                    return false;
                };
                builder.build_as_child(window)
            };
            match built {
                Ok(webview) => {
                    #[cfg(target_os = "macos")]
                    {
                        use wry::WebViewExtMacOS as _;
                        // WKWebView disables trackpad magnification by default.
                        // Enabling the native gesture preserves the trackpad's
                        // focal point and momentum inside the vector preview.
                        unsafe {
                            webview.webview().setAllowsMagnification(true);
                        }
                    }
                    self.webview = Some(webview);
                    self.webview_url = Some(url.clone());
                    self.webview_state =
                        ServiceState::Ready("Tinymist vector frontend is embedded".to_owned());
                }
                Err(error) => {
                    self.webview_state = ServiceState::Failed(format!(
                        "Could not embed the Tinymist preview: {error}"
                    ));
                    return false;
                }
            }
        }
        if self.webview_url.as_deref() != Some(url.as_str()) {
            if let Some(webview) = &self.webview
                && let Err(error) = webview.load_url(&url)
            {
                self.webview = None;
                self.webview_url = None;
                self.webview_state =
                    ServiceState::Failed(format!("Could not load the Tinymist preview: {error}"));
                return false;
            }
            self.webview_url = Some(url);
        }
        if let Some(webview) = &self.webview {
            let _ = webview.set_background_color((
                background.r(),
                background.g(),
                background.b(),
                background.a(),
            ));
            if let Err(error) = webview.set_bounds(bounds) {
                self.webview_state =
                    ServiceState::Failed(format!("Could not position the vector preview: {error}"));
                self.webview = None;
                self.webview_url = None;
                return false;
            }
            if let Err(error) = webview.set_visible(visible) {
                self.webview_state =
                    ServiceState::Failed(format!("Could not show the vector preview: {error}"));
                self.webview = None;
                self.webview_url = None;
                return false;
            }
        }
        true
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn update_webview(
        &mut self,
        _context: &egui::Context,
        _frame: &mut eframe::Frame,
        _rect: Rect,
        _background: Color32,
        _visible: bool,
    ) -> bool {
        false
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn hide_webview(&self) {
        if let Some(webview) = &self.webview {
            let _ = webview.set_visible(false);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn hide_webview(&self) {}

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn refresh_native_window_parent(&mut self, context: &egui::Context) {
        if context.input(|input| input.viewport().focused) != Some(true) {
            return;
        }
        let Some(parent) = crate::native_window::active_window_handle() else {
            return;
        };
        let changed = self
            .native_window_parent
            .as_ref()
            .is_some_and(|current| !current.is_same_window(&parent));
        if changed {
            // A recreated native viewport needs a fresh child WKWebView. The
            // Tinymist process and preview URL remain owned by this session.
            self.webview = None;
            self.webview_url = None;
            self.webview_state =
                ServiceState::Starting("Attaching preview to this document window".to_owned());
        }
        self.native_window_parent = Some(parent);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn refresh_native_window_parent(&mut self, _context: &egui::Context) {}

    pub(crate) fn take_settings_update(&mut self) -> Option<AppSettings> {
        self.pending_settings.take()
    }

    pub(crate) fn apply_shared_settings(&mut self, settings: AppSettings, context: &egui::Context) {
        let autosave_changed = settings.auto_save != self.settings.auto_save
            || settings.auto_save_delay_ms != self.settings.auto_save_delay_ms;
        self.settings = settings;
        if autosave_changed {
            self.autosave_deadline =
                (self.settings.auto_save && self.path.is_some() && self.is_dirty()).then(|| {
                    Instant::now()
                        + Duration::from_millis(self.settings.auto_save_delay_ms.max(100))
                });
        }
        context.request_repaint();
    }
}

impl eframe::App for EditorApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn raw_input_hook(&mut self, context: &egui::Context, _raw_input: &mut egui::RawInput) {
        let Some(settings) = self.pending_settings.take() else {
            return;
        };
        self.apply_shared_settings(settings, context);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.pending_settings
            .as_ref()
            .unwrap_or(&self.settings)
            .save(storage);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        // Capture the exact platform parent before opening any popup viewport
        // or consuming native menu commands. Each EditorApp retains its own
        // parent and consequently its own Tinymist child webview.
        self.refresh_native_window_parent(&context);
        if self.captures.has_pending_for("settings") {
            self.settings_visible = true;
        }
        // Keep a multi-window QA batch out of its own screenshots. Results are
        // still printed immediately, then surfaced in the UI once the last
        // requested viewport has completed.
        if !self.captures.has_pending() {
            for result in self.captures.drain_results() {
                self.notice = Some(match result.error {
                    Some(error) => Notice {
                        message: format!("UI screenshot failed: {error}"),
                        kind: NoticeKind::Error,
                    },
                    None => Notice {
                        message: format!("UI screenshot saved to {}", result.path.display()),
                        kind: NoticeKind::Success,
                    },
                });
            }
        }
        install_hover_runtime_config(
            &context,
            Duration::from_millis(self.settings.hover_delay_ms),
            Duration::from_millis(self.settings.hover_fade_ms),
        );
        if context.input(|input| input.viewport().focused == Some(true)) {
            if self.app_modal_suspended {
                self.app_modal_had_focus = false;
                self.app_modal_suspended = false;
            }
            if self.rename_overlay_suspended {
                self.rename_overlay_had_focus = false;
                self.rename_overlay_suspended = false;
            }
        }
        // The GL surface is alpha-capable for child popup viewports. Keep the
        // main window itself fully opaque by painting its complete root first.
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let native_tooltip_id = native_hover_tooltip_id(&context);
        let geometry_id = tooltip_geometry_id(&context);
        let pointer = context
            .pointer_hover_pos()
            .or_else(|| context.pointer_latest_pos());
        let tooltip_retained = context.data(|data| {
            data.get_temp::<TooltipGeometry>(geometry_id)
                .is_some_and(|geometry| {
                    if geometry.pointer_inside_card {
                        return true;
                    }
                    pointer.is_some_and(|pointer| {
                        tooltip_region_contains(pointer, geometry.origin, geometry.card)
                    })
                })
        });
        if !tooltip_retained {
            self.diagnostic_tooltip = None;
            context.data_mut(|data| {
                data.remove::<HoverTooltipOverlay>(native_tooltip_id);
                data.remove::<TooltipGeometry>(geometry_id);
            });
        }
        self.receive_open_requests(&context);
        self.receive_native_menu_commands(&context, frame);
        self.execute_pending_app_popup_action(&context, frame);
        self.execute_pending_document_action(&context, frame);
        self.sync_runtime_settings(&context);
        self.receive_compile_results(&context);
        self.receive_asset_results(&context);
        self.poll_export_dialog(&context);
        self.poll_tool_picker(&context);
        self.poll_document_dialog(&context);
        self.receive_tinymist_events(&context);
        self.receive_web_links();
        self.handle_shortcuts(&context, frame);
        self.handle_dropped_file(&context);
        self.handle_close_request(&context);
        self.update_title(&context);
        self.tick_workspace(&context);
        self.tick_project_index(&context);
        self.apply_snapshot_scene();
        self.record_status_transition();

        egui::Panel::top("toolbar")
            .exact_size(METRICS.chrome.toolbar_height)
            .show(ui, |ui| self.show_toolbar(ui, frame));
        egui::Panel::bottom("status-bar")
            .exact_size(METRICS.chrome.status_height)
            .show(ui, |ui| self.show_status_bar(ui));
        if self.problems_visible {
            let panel = egui::Panel::bottom("problems");
            let panel = if self.snapshot_scene == Some(UiSnapshotScene::ProblemsPanel) {
                panel.resizable(false).exact_size(220.0)
            } else {
                panel
                    .resizable(true)
                    .default_size(METRICS.chrome.problems_default_height)
                    .min_size(METRICS.chrome.problems_min_height)
                    .max_size(METRICS.chrome.problems_max_height)
            };
            panel.show(ui, |ui| self.show_problems(ui));
        }
        if self.filesystem_visible {
            egui::Panel::left("filesystem")
                .frame(theme::content_panel_frame(ui.style()))
                .resizable(true)
                .default_size(METRICS.chrome.explorer_default_width)
                .min_size(METRICS.chrome.explorer_min_width)
                .show(ui, |ui| self.show_workspace(ui));
        }
        if self.document_kind.preview_only() {
            // This presentation override intentionally does not mutate
            // `view_mode`: returning to a source file restores the user's Code,
            // Split, or Preview preference.
            egui::CentralPanel::default()
                .frame(theme::content_panel_frame(ui.style()))
                .show(ui, |ui| self.show_preview(ui, frame));
        } else if self.document_kind == DocumentKind::Text {
            self.hide_webview();
            egui::CentralPanel::default()
                .frame(theme::content_panel_frame(ui.style()))
                .show(ui, |ui| self.show_editor(ui));
        } else {
            match self.view_mode {
                ViewMode::Code => {
                    self.hide_webview();
                    egui::CentralPanel::default()
                        .frame(theme::content_panel_frame(ui.style()))
                        .show(ui, |ui| self.show_editor(ui));
                }
                ViewMode::Split => {
                    let available = ui.available_width();
                    let preview_reserve = METRICS.chrome.split_preview_reserve.min(
                        (available * METRICS.chrome.split_preview_fraction)
                            .max(METRICS.chrome.split_pane_hard_minimum),
                    );
                    let max_editor =
                        (available - preview_reserve).max(METRICS.chrome.split_pane_hard_minimum);
                    let min_editor = METRICS.chrome.split_editor_minimum.min(max_editor);
                    let editor_width = (available * METRICS.chrome.split_editor_fraction)
                        .clamp(min_editor, max_editor);
                    egui::Panel::left("editor")
                        .frame(theme::content_panel_frame(ui.style()))
                        .resizable(true)
                        .default_size(editor_width)
                        .min_size(min_editor)
                        .max_size(max_editor)
                        .show(ui, |ui| self.show_editor(ui));
                    egui::CentralPanel::default()
                        .frame(theme::content_panel_frame(ui.style()))
                        .show(ui, |ui| self.show_preview(ui, frame));
                }
                ViewMode::Preview => {
                    egui::CentralPanel::default()
                        .frame(theme::content_panel_frame(ui.style()))
                        .show(ui, |ui| self.show_preview(ui, frame));
                }
            }
        }
        self.show_app_popup_window(&context);
        self.show_rename_dialog(&context);
        self.show_app_modal_window(&context);
        self.show_diagnostic_tooltip_window(&context);
        self.show_settings_window(&context, frame);
        self.show_typst_overrides_window(&context);
        self.show_workspace_chooser(&context);
        self.sync_preview_visibility();
        self.tick_autosave(&context);
        self.tick_compile(&context);
    }
}

fn make_preview_texture(
    context: &egui::Context,
    revision: u64,
    index: usize,
    page: PreviewPage,
    dark: bool,
) -> PreviewTexture {
    let PreviewPage { size, rgba, links } = page;
    let pixels = if dark {
        dark_preview_rgba(&rgba)
    } else {
        rgba.clone()
    };
    let texture = context.load_texture(
        format!("preview-{revision}-{index}-{dark}"),
        preview_color_image(size, &pixels),
        TextureOptions::LINEAR,
    );
    PreviewTexture {
        size,
        raster_size: size,
        rgba,
        links,
        texture,
    }
}

fn preview_color_image(size: [usize; 2], rgba: &[u8]) -> ColorImage {
    ColorImage::from_rgba_unmultiplied(size, rgba)
}

fn take_matching_export(
    pending: &mut Option<PendingExport>,
    document_epoch: u64,
) -> Option<PathBuf> {
    pending
        .take()
        .filter(|export| export.document_epoch == document_epoch)
        .map(|export| export.path)
}

fn paint_editor_line_backgrounds(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    source: &str,
    current_char: Option<usize>,
    attention: Option<(usize, f32)>,
    line_rows: &[Range<usize>],
    slot: egui::layers::ShapeIdx,
) {
    let mut shapes = Vec::new();

    if let Some(char_index) = current_char {
        let line = line_index_at_char(source, char_index);
        let fill = theme::palette(ui.visuals().dark_mode).active_row;
        push_editor_line_fill(&mut shapes, output, line_rows, line, fill, None);
    }

    if let Some((char_index, progress)) = attention {
        let cursor = output
            .galley
            .pos_from_cursor(CCursor::new(char_index.min(source.chars().count())))
            .translate(output.galley_pos.to_vec2());
        let center = Pos2::new(cursor.left(), cursor.center().y);
        let fade = (1.0 - progress).powi(2);
        let radius = METRICS.editor.attention_start_radius
            + METRICS.editor.attention_radius_growth * progress;
        let palette = theme::palette(ui.visuals().dark_mode);
        let [red, green, blue] = palette.attention_rgb;
        let center_color = Color32::from_rgba_unmultiplied(
            red,
            green,
            blue,
            (palette.attention_max_alpha as f32 * fade) as u8,
        );
        let mut mesh = egui::epaint::Mesh::default();
        mesh.colored_vertex(center, center_color);
        for index in 0..METRICS.editor.attention_segments {
            let angle =
                index as f32 / METRICS.editor.attention_segments as f32 * std::f32::consts::TAU;
            mesh.colored_vertex(center + Vec2::angled(angle) * radius, Color32::TRANSPARENT);
        }
        for index in 0..METRICS.editor.attention_segments {
            mesh.add_triangle(
                0,
                index + 1,
                ((index + 1) % METRICS.editor.attention_segments) + 1,
            );
        }
        shapes.push(egui::Shape::mesh(mesh));
        let ring_color = center_color.gamma_multiply(METRICS.editor.attention_ring_opacity);
        shapes.push(egui::Shape::circle_stroke(
            center,
            METRICS.editor.attention_ring_start_radius
                + METRICS.editor.attention_ring_growth * progress,
            Stroke::new(METRICS.editor.attention_ring_width, ring_color),
        ));
    }

    ui.painter()
        .with_clip_rect(output.text_clip_rect)
        .set(slot, egui::Shape::Vec(shapes));
}

fn push_editor_line_fill(
    shapes: &mut Vec<egui::Shape>,
    output: &egui::text_edit::TextEditOutput,
    line_rows: &[Range<usize>],
    line: usize,
    fill: Color32,
    marker: Option<Color32>,
) {
    let Some(rows) = line_rows.get(line) else {
        return;
    };
    let mut combined = Rect::NOTHING;
    for row in &output.galley.rows[rows.clone()] {
        let row_rect = row.rect().translate(output.galley_pos.to_vec2());
        let rect = Rect::from_min_max(
            Pos2::new(output.response.rect.left(), row_rect.top()),
            Pos2::new(output.response.rect.right(), row_rect.bottom()),
        );
        combined = combined.union(rect);
        shapes.push(egui::Shape::rect_filled(rect, 0.0, fill));
    }
    if let Some(marker) = marker
        && combined.is_positive()
    {
        let marker_rect = Rect::from_min_max(
            combined.left_top(),
            Pos2::new(
                combined.left() + METRICS.editor.diagnostic_marker_width,
                combined.bottom(),
            ),
        );
        shapes.push(egui::Shape::rect_filled(
            marker_rect,
            METRICS.editor.diagnostic_marker_radius,
            marker,
        ));
    }
}

fn line_index_at_char(source: &str, char_index: usize) -> usize {
    source
        .chars()
        .take(char_index)
        .filter(|character| *character == '\n')
        .count()
}

fn line_column_at_char(source: &str, char_index: usize) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;
    for character in source.chars().take(char_index) {
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn typst_hover_token_range(source: &str, char_index: usize) -> Option<Range<usize>> {
    let is_identifier =
        |character: char| character == '_' || character == '-' || character.is_alphanumeric();
    let (mut index, (mut cursor_byte, mut character)) = source
        .char_indices()
        .nth(char_index)
        .map(|selected| (char_index, selected))
        .or_else(|| {
            source
                .char_indices()
                .next_back()
                .map(|selected| (source.chars().count().saturating_sub(1), selected))
        })?;
    if !is_identifier(character) {
        let previous = source[..cursor_byte].char_indices().next_back();
        if matches!(character, '(' | ')' | '.')
            && let Some((previous_byte, previous_character)) = previous
            && is_identifier(previous_character)
        {
            index = index.saturating_sub(1);
            cursor_byte = previous_byte;
            character = previous_character;
        } else {
            return None;
        }
    }
    let mut start = index;
    for (_, previous_character) in source[..cursor_byte].char_indices().rev() {
        if !is_identifier(previous_character) {
            break;
        }
        start = start.saturating_sub(1);
    }
    let mut end = index + 1;
    let mut suffix = &source[cursor_byte + character.len_utf8()..];
    while let Some((offset, next_character)) = suffix.char_indices().next() {
        if !is_identifier(next_character) {
            break;
        }
        end += 1;
        suffix = &suffix[offset + next_character.len_utf8()..];
    }
    (start < end).then_some(start..end)
}

fn editor_attention_progress(elapsed: Duration) -> f32 {
    (elapsed.as_secs_f32() / METRICS.motion.editor_attention.as_secs_f32()).clamp(0.0, 1.0)
}

fn editor_surface_rect(editor_rect: Rect, viewport_rect: Rect) -> Rect {
    editor_rect.union(viewport_rect)
}

fn paint_line_diagnostics(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    diagnostics: &[LineDiagnostic],
    line_rows: &[Range<usize>],
    slots: Vec<egui::layers::ShapeIdx>,
) -> Option<DiagnosticTooltipOverlay> {
    let painter = ui.painter();
    let mut hovered_diagnostic = None;
    for (diagnostic, slot) in diagnostics.iter().zip(slots) {
        let Some(rows) = line_rows.get(diagnostic.line.saturating_sub(1)) else {
            continue;
        };
        let color = diagnostic_color(diagnostic.severity, ui.visuals().dark_mode);
        let mut background = Vec::new();
        let mut hover_rect = Rect::NOTHING;
        for row in &output.galley.rows[rows.clone()] {
            let row_rect = row.rect().translate(output.galley_pos.to_vec2());
            let line_rect = Rect::from_min_max(
                Pos2::new(output.response.rect.left(), row_rect.top()),
                Pos2::new(output.response.rect.right(), row_rect.bottom()),
            );
            hover_rect = hover_rect.union(line_rect);
            background.push(egui::Shape::rect_filled(
                line_rect,
                0.0,
                color.gamma_multiply(METRICS.editor.diagnostic_background_opacity),
            ));
        }
        painter.set(slot, egui::Shape::Vec(background));

        let Some(row) = output.galley.rows.get(rows.end.saturating_sub(1)) else {
            continue;
        };
        let row_rect = row.rect().translate(output.galley_pos.to_vec2());
        let text = format!("  {}", diagnostic.summary);
        let galley = painter.layout_no_wrap(text, theme::annotation_font(), color);
        let annotation_pos = Pos2::new(
            output.galley_pos.x + row.rect().right() + METRICS.editor.annotation_gap,
            row_rect.top(),
        );
        let annotation_rect = Rect::from_min_size(annotation_pos, galley.size());
        hover_rect = hover_rect.union(annotation_rect);
        painter.galley(annotation_pos, galley, color);
        let response = ui.interact(
            hover_rect,
            output.response.id.with(("diagnostic", diagnostic.line)),
            Sense::hover(),
        );
        if let Some(opacity) = hover_opacity(&response, diagnostic_hover_timing_id(&response.ctx))
            && hovered_diagnostic.is_none()
        {
            hovered_diagnostic = Some(DiagnosticTooltipOverlay {
                origin: hover_rect,
                anchor: Pos2::new(
                    output.response.rect.right() + METRICS.editor.tooltip_gap,
                    hover_rect.top(),
                ),
                severity: diagnostic.severity,
                detail: diagnostic.detail.clone(),
                opacity,
            });
        }
    }
    hovered_diagnostic
}

fn paint_line_numbers(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    line_rows: &[Range<usize>],
) {
    let painter = ui.painter();
    let right = output.galley_pos.x - METRICS.editor.line_number_right_gap;
    let separator_x = output.galley_pos.x - METRICS.editor.line_number_separator_gap;
    let separator = Stroke::new(
        METRICS.editor.line_number_separator_width,
        ui.visuals().widgets.noninteractive.bg_stroke.color,
    );
    painter.line_segment(
        [
            Pos2::new(separator_x, output.response.rect.top()),
            Pos2::new(separator_x, output.response.rect.bottom()),
        ],
        separator,
    );
    let color = ui.visuals().weak_text_color();
    for (line, rows) in line_rows.iter().enumerate() {
        let Some(row) = output.galley.rows.get(rows.start) else {
            continue;
        };
        let row_rect = row.rect().translate(output.galley_pos.to_vec2());
        painter.text(
            Pos2::new(right, row_rect.top()),
            egui::Align2::RIGHT_TOP,
            (line + 1).to_string(),
            theme::annotation_font(),
            color,
        );
    }
}

fn logical_line_count(source: &str) -> usize {
    source.bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn line_number_gutter_width(line_count: usize, enabled: bool) -> i8 {
    if !enabled {
        return METRICS.editor.gutter_disabled_width;
    }
    let digits = line_count.max(1).ilog10() + 1;
    ((digits * METRICS.editor.gutter_digit_width + METRICS.editor.gutter_base_width)
        .min(METRICS.editor.gutter_max_width)) as i8
}

fn logical_line_row_ranges(rows: &[egui::epaint::text::PlacedRow]) -> Vec<Range<usize>> {
    logical_line_row_ranges_from_breaks(rows.iter().map(|row| row.ends_with_newline))
}

fn logical_line_row_ranges_from_breaks(
    breaks: impl IntoIterator<Item = bool>,
) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for ends_with_newline in breaks {
        count += 1;
        if ends_with_newline {
            ranges.push(start..count);
            start = count;
        }
    }
    if start < count {
        ranges.push(start..count);
    }
    ranges
}

fn add_workspace_nodes(
    builder: &mut TreeViewBuilder<'_, PathBuf>,
    nodes: &[WorkspaceNode],
    active: Option<&Path>,
    preview: Option<&Path>,
) {
    for node in nodes {
        let is_active = active.is_some_and(|path| same_path(path, &node.path));
        let is_preview = preview.is_some_and(|path| same_path(path, &node.path));
        let label = node.display_name().into_owned();
        if node.is_directory() {
            let open = builder.node(
                NodeBuilder::dir(node.path.clone())
                    .default_open(active.is_some_and(|path| path.starts_with(&node.path)))
                    .icon(|ui| paint_tree_icon(ui, true))
                    .label_ui(move |ui| {
                        let text = RichText::new(&label);
                        ui.label(if is_active { text.strong() } else { text });
                    }),
            );
            if open {
                add_workspace_nodes(builder, &node.children, active, preview);
            }
            builder.close_dir();
        } else if node.is_file() {
            builder.node(
                NodeBuilder::leaf(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, false))
                    .label_ui(move |ui| {
                        ui.horizontal(|ui| {
                            let text = RichText::new(&label);
                            ui.label(if is_active { text.strong() } else { text });
                            if is_preview {
                                let blue = theme::palette(ui.visuals().dark_mode).accent;
                                native_hover_text(
                                    static_icon(ui, UiIcon::Eye, blue),
                                    "Used for preview",
                                );
                            }
                        });
                    }),
            );
        } else if node.is_symlink() {
            builder.node(
                NodeBuilder::leaf(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, false))
                    .label_ui(move |ui| {
                        let text = RichText::new(format!("{label} (link)"));
                        ui.label(if is_active { text.strong() } else { text });
                    }),
            );
        }
    }
}

const EXPLORER_SECTION_SPECS: [(&str, bool); 5] = [
    ("workspace-files", true),
    ("workspace-contents", true),
    ("workspace-subfiles", false),
    ("workspace-symbols", false),
    ("workspace-packages", false),
];

fn explorer_section_state_id(ui: &egui::Ui, id_salt: &'static str) -> egui::Id {
    ui.make_persistent_id(("explorer-section", id_salt))
}

fn explorer_section_body_height(ui: &egui::Ui) -> f32 {
    let open_sections = EXPLORER_SECTION_SPECS
        .iter()
        .filter(|(id_salt, default_open)| {
            egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                explorer_section_state_id(ui, id_salt),
                *default_open,
            )
            .is_open()
        })
        .count();
    let frame_height = theme::explorer_section_frame(ui.style())
        .total_margin()
        .sum()
        .y;
    available_explorer_section_body_height(ui.available_height(), open_sections, frame_height)
}

fn available_explorer_section_body_height(
    available_height: f32,
    open_sections: usize,
    frame_height: f32,
) -> f32 {
    if open_sections == 0 {
        return 0.0;
    }
    let section_count = EXPLORER_SECTION_SPECS.len() as f32;
    let reserved_headers = section_count * (METRICS.explorer.section_header_height + frame_height);
    let reserved_gaps = (section_count - 1.0).max(0.0) * METRICS.explorer.section_gap;
    ((available_height - reserved_headers - reserved_gaps) / open_sections as f32).max(0.0)
}

fn explorer_section(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    title: &'static str,
    default_open: bool,
    body_height: f32,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        explorer_section_state_id(ui, id_salt),
        default_open,
    );
    theme::explorer_section_frame(ui.style()).show(ui, move |ui| {
        ui.set_width(ui.available_width().max(0.0));
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut title_clicked = false;
        let mut header = state.show_header(ui, |ui| {
            let response = ui.add_sized(
                [
                    ui.available_width().max(0.0),
                    METRICS.explorer.section_header_height,
                ],
                egui::Button::new(RichText::new(title).strong()).frame(false),
            );
            title_clicked = response.clicked();
        });
        if title_clicked {
            header.toggle();
        }
        header.body_unindented(|ui| {
            egui::ScrollArea::both()
                .id_salt((id_salt, "scroll"))
                .max_width(ui.available_width().max(0.0))
                .max_height(body_height)
                .min_scrolled_width(0.0)
                .min_scrolled_height(0.0)
                .auto_shrink([false, false])
                .show(ui, add_body);
        });
    });
}

fn show_project_index_sections(
    ui: &mut egui::Ui,
    root: &Path,
    index: &ProjectIndex,
    body_height: f32,
    target: &mut Option<(PathBuf, usize)>,
) {
    explorer_section(
        ui,
        "workspace-contents",
        "Contents",
        true,
        body_height,
        |ui| {
            if index.outline.is_empty() {
                ui.label(RichText::new("No headings").small().weak());
                return;
            }
            for entry in &index.outline {
                let indent = entry
                    .level
                    .saturating_sub(1)
                    .min(METRICS.explorer.outline_max_depth) as f32
                    * METRICS.explorer.outline_indent;
                let location = format!(
                    "{}:{}",
                    project_relative_path(root, &entry.path),
                    entry.line
                );
                let response =
                    explorer_index_row(ui, &entry.title, Some(&entry.line.to_string()), indent);
                if native_hover_text(response, location).clicked() {
                    *target = Some((entry.path.clone(), entry.line));
                }
            }
        },
    );

    explorer_section(
        ui,
        "workspace-subfiles",
        "Subfiles",
        false,
        body_height,
        |ui| {
            if index.subfiles.is_empty() {
                ui.label(RichText::new("No included files").small().weak());
                return;
            }
            for path in &index.subfiles {
                let label = project_relative_path(root, path);
                let response = explorer_index_row(ui, &label, None, 0.0);
                if native_hover_text(response, path.display().to_string()).clicked() {
                    *target = Some((path.clone(), 1));
                }
            }
        },
    );

    explorer_section(
        ui,
        "workspace-symbols",
        "Symbols",
        false,
        body_height,
        |ui| {
            if index.symbols.is_empty() {
                ui.label(RichText::new("No definitions or functions").small().weak());
                return;
            }
            for symbol in &index.symbols {
                let kind = symbol.kind.label();
                let location = format!(
                    "{}:{} · {kind}",
                    project_relative_path(root, &symbol.path),
                    symbol.line
                );
                let response = explorer_index_row(ui, &symbol.name, Some(kind), 0.0);
                if native_hover_text(response, location).clicked() {
                    *target = Some((symbol.path.clone(), symbol.line));
                }
            }
        },
    );

    explorer_section(
        ui,
        "workspace-packages",
        "Packages",
        false,
        body_height,
        |ui| {
            if index.packages.is_empty() {
                ui.label(RichText::new("No packages").small().weak());
            } else {
                for package in &index.packages {
                    explorer_index_row(ui, package, None, 0.0);
                }
            }
        },
    );
}

fn project_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn explorer_index_row(
    ui: &mut egui::Ui,
    label: &str,
    detail: Option<&str>,
    indent: f32,
) -> egui::Response {
    let size = Vec2::new(ui.available_width().max(1.0), METRICS.explorer.row_height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let visuals = ui.style().interact(&response);
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, theme::RADIUS.row as f32, visuals.weak_bg_fill);
    }

    let painter = ui.painter().with_clip_rect(rect);
    let font = theme::supporting_font();
    let left = rect.left() + theme::SPACE.small + indent;
    let show_detail = detail.is_some() && rect.width() >= METRICS.explorer.detail_breakpoint;
    let detail_width = if show_detail {
        METRICS.explorer.detail_width
    } else {
        0.0
    };
    let label_clip = Rect::from_min_max(
        Pos2::new(left, rect.top()),
        Pos2::new(
            (rect.right() - detail_width - theme::SPACE.small).max(left),
            rect.bottom(),
        ),
    );
    painter.with_clip_rect(label_clip).text(
        Pos2::new(left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font.clone(),
        visuals.fg_stroke.color,
    );
    if show_detail {
        painter.text(
            Pos2::new(rect.right() - theme::SPACE.small, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            detail.unwrap_or_default(),
            font,
            ui.visuals().weak_text_color(),
        );
    }
    response
}

fn paint_tree_icon(ui: &mut egui::Ui, folder: bool) {
    let rect = ui.available_rect_before_wrap().shrink(theme::SPACE.tight);
    let size = METRICS.explorer.tree_icon_size.min(rect.size());
    let rect = Rect::from_center_size(rect.center(), size);
    let color = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let stroke = Stroke::new(METRICS.explorer.tree_icon_stroke, color);
    if folder {
        let body = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + 3.0),
            Pos2::new(rect.right(), rect.bottom()),
        );
        ui.painter()
            .rect_stroke(body, 1.5, stroke, StrokeKind::Inside);
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 1.5, rect.top() + 3.0),
                Pos2::new(rect.left() + 4.5, rect.top()),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 4.5, rect.top()),
                Pos2::new(rect.left() + 8.0, rect.top() + 3.0),
            ],
            stroke,
        );
    } else {
        ui.painter()
            .rect_stroke(rect, 1.2, stroke, StrokeKind::Inside);
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 3.0, rect.top() + 4.0),
                Pos2::new(rect.right() - 3.0, rect.top() + 4.0),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 3.0, rect.top() + 7.0),
                Pos2::new(rect.right() - 3.0, rect.top() + 7.0),
            ],
            stroke,
        );
    }
}

fn icon_button(ui: &mut egui::Ui, icon: UiIcon, tooltip: &str) -> egui::Response {
    icon_button_enabled(ui, true, icon, tooltip)
}

fn icon_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: UiIcon,
    tooltip: &str,
) -> egui::Response {
    let response = native_hover_text(
        ui.add_enabled(
            enabled,
            egui::Button::new("").min_size(METRICS.icon.button_size),
        ),
        tooltip,
    );
    let color = ui.style().interact(&response).fg_stroke.color;
    paint_ui_icon(
        ui.painter(),
        response.rect.shrink(METRICS.icon.button_icon_shrink),
        icon,
        color,
    );
    response
}

fn static_icon(ui: &mut egui::Ui, icon: UiIcon, color: Color32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(METRICS.icon.static_size), Sense::hover());
    paint_ui_icon(
        ui.painter(),
        rect.shrink(METRICS.icon.static_shrink),
        icon,
        color,
    );
    response
}

fn paint_ui_icon(painter: &egui::Painter, rect: Rect, icon: UiIcon, color: Color32) {
    let center = rect.center();
    let stroke = Stroke::new(METRICS.icon.stroke_width, color);
    match icon {
        UiIcon::Check => {
            painter.line_segment(
                [
                    Pos2::new(rect.left() + rect.width() * 0.12, center.y),
                    Pos2::new(rect.left() + rect.width() * 0.42, rect.bottom() - 2.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left() + rect.width() * 0.42, rect.bottom() - 2.0),
                    Pos2::new(rect.right() - 1.0, rect.top() + 2.0),
                ],
                stroke,
            );
        }
        UiIcon::Close => {
            painter.line_segment([rect.left_top(), rect.right_bottom()], stroke);
            painter.line_segment([rect.right_top(), rect.left_bottom()], stroke);
        }
        UiIcon::Up | UiIcon::Previous => {
            let (a, b, c) = if icon == UiIcon::Up {
                (
                    Pos2::new(rect.left() + 1.0, rect.bottom() - 2.0),
                    Pos2::new(center.x, rect.top() + 2.0),
                    Pos2::new(rect.right() - 1.0, rect.bottom() - 2.0),
                )
            } else {
                (
                    Pos2::new(rect.right() - 2.0, rect.top() + 1.0),
                    Pos2::new(rect.left() + 2.0, center.y),
                    Pos2::new(rect.right() - 2.0, rect.bottom() - 1.0),
                )
            };
            painter.line_segment([a, b], stroke);
            painter.line_segment([b, c], stroke);
        }
        UiIcon::Down | UiIcon::Next => {
            let (a, b, c) = if icon == UiIcon::Down {
                (
                    Pos2::new(rect.left() + 1.0, rect.top() + 2.0),
                    Pos2::new(center.x, rect.bottom() - 2.0),
                    Pos2::new(rect.right() - 1.0, rect.top() + 2.0),
                )
            } else {
                (
                    Pos2::new(rect.left() + 2.0, rect.top() + 1.0),
                    Pos2::new(rect.right() - 2.0, center.y),
                    Pos2::new(rect.left() + 2.0, rect.bottom() - 1.0),
                )
            };
            painter.line_segment([a, b], stroke);
            painter.line_segment([b, c], stroke);
        }
        UiIcon::Refresh => {
            // Draw a real open arc with a tangent arrowhead. The old icon drew
            // an arrow over a closed circle, which let the circle obscure the
            // head at small UI scales.
            let radius = rect.width().min(rect.height()) * 0.34;
            let end_angle = -0.45_f32;
            let start_angle = end_angle - 5.0;
            let points = (0..=18)
                .map(|index| {
                    let angle = start_angle + (end_angle - start_angle) * index as f32 / 18.0;
                    center + Vec2::new(angle.cos(), angle.sin()) * radius
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
            let tip = center + Vec2::new(end_angle.cos(), end_angle.sin()) * radius;
            let tangent = Vec2::new(-end_angle.sin(), end_angle.cos()).normalized();
            let normal = Vec2::new(-tangent.y, tangent.x);
            painter.line_segment([tip, tip - tangent * 4.0 + normal * 2.5], stroke);
            painter.line_segment([tip, tip - tangent * 4.0 - normal * 2.5], stroke);
        }
        UiIcon::Waiting => {
            painter.circle_stroke(center, rect.width().min(rect.height()) * 0.36, stroke);
        }
        UiIcon::Eye => {
            let half_width = rect.width() * 0.44;
            let half_height = rect.height() * 0.25;
            let left = Pos2::new(center.x - half_width, center.y);
            let right = Pos2::new(center.x + half_width, center.y);
            let top = Pos2::new(center.x, center.y - half_height);
            let bottom = Pos2::new(center.x, center.y + half_height);
            painter.add(egui::Shape::line(vec![left, top, right], stroke));
            painter.add(egui::Shape::line(vec![left, bottom, right], stroke));
            painter.circle_filled(center, rect.width().min(rect.height()) * 0.10, color);
        }
        UiIcon::ZoomIn | UiIcon::ZoomOut => {
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 1.0, center.y),
                    Pos2::new(rect.right() - 1.0, center.y),
                ],
                stroke,
            );
            if icon == UiIcon::ZoomIn {
                painter.line_segment(
                    [
                        Pos2::new(center.x, rect.top() + 1.0),
                        Pos2::new(center.x, rect.bottom() - 1.0),
                    ],
                    stroke,
                );
            }
        }
        UiIcon::FitWidth => {
            painter.line_segment(
                [
                    Pos2::new(rect.left(), rect.top()),
                    Pos2::new(rect.left(), rect.bottom()),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.right(), rect.top()),
                    Pos2::new(rect.right(), rect.bottom()),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 3.0, center.y),
                    Pos2::new(rect.right() - 3.0, center.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 3.0, center.y),
                    Pos2::new(rect.left() + 6.0, center.y - 3.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.right() - 3.0, center.y),
                    Pos2::new(rect.right() - 6.0, center.y - 3.0),
                ],
                stroke,
            );
        }
        UiIcon::Warning => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(center.x, rect.top()),
                    rect.right_bottom(),
                    rect.left_bottom(),
                ],
                Color32::TRANSPARENT,
                stroke,
            ));
            painter.line_segment(
                [
                    Pos2::new(center.x, rect.top() + 4.0),
                    Pos2::new(center.x, rect.bottom() - 4.0),
                ],
                stroke,
            );
            painter.circle_filled(Pos2::new(center.x, rect.bottom() - 2.0), 1.0, color);
        }
    }
}

fn clipped_panel_content_ui(ui: &mut egui::Ui, id_salt: &'static str) -> egui::Ui {
    let viewport = ui.available_rect_before_wrap();
    let clip_rect = ui.clip_rect().intersect(viewport);
    ui.allocate_rect(viewport, Sense::hover());
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id_salt)
            .max_rect(viewport)
            .layout(Layout::top_down(Align::Min)),
    );
    content.set_clip_rect(clip_rect);
    content
}

fn preview_background(ui: &egui::Ui) -> Color32 {
    ui.visuals().panel_fill
}

fn show_preview_transition(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    ui.painter().rect_filled(rect, 0.0, preview_background(ui));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Center)),
        |ui| {
            ui.add_space(
                (rect.height() * METRICS.preview.transition_vertical_fraction)
                    .max(METRICS.preview.transition_min_top_space),
            );
            ui.spinner();
            ui.label(RichText::new("Updating preview…").small().weak());
        },
    );
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn same_web_origin(base: &str, candidate: &str) -> bool {
    let (Ok(base), Ok(candidate)) = (url::Url::parse(base), url::Url::parse(candidate)) else {
        return false;
    };
    base.scheme() == candidate.scheme()
        && base.host_str() == candidate.host_str()
        && base.port_or_known_default() == candidate.port_or_known_default()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn preview_document_file_url(
    candidate: &str,
    project_root: &Path,
    source_dir: Option<&Path>,
) -> Option<String> {
    let candidate_url = url::Url::parse(candidate).ok()?;
    let decoded_path = percent_decode_url_path(candidate_url.path())?;
    let relative = Path::new(decoded_path.trim_start_matches('/'));
    if relative.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return None;
    }
    let filename = relative.file_name().map(PathBuf::from);
    let mut candidates = vec![project_root.join(relative)];
    if let Some(source_dir) = source_dir {
        candidates.push(source_dir.join(relative));
        if let Some(filename) = filename {
            candidates.push(source_dir.join(filename));
        }
    }
    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let path = candidates.into_iter().find_map(|path| {
        let path = path.canonicalize().ok()?;
        (path.is_file() && path.starts_with(&canonical_root) && DocumentKind::supports_path(&path))
            .then_some(path)
    })?;
    let mut file_url = url::Url::from_file_path(path).ok()?;
    file_url.set_query(candidate_url.query());
    file_url.set_fragment(candidate_url.fragment());
    Some(file_url.into())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn percent_decode_url_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push(hex_digit(high)? << 4 | hex_digit(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn pdf_page_from_url(url: &url::Url) -> Option<usize> {
    let from_query = url
        .query_pairs()
        .find(|(key, _)| key.eq_ignore_ascii_case("page"))
        .and_then(|(_, value)| value.parse::<usize>().ok());
    let from_fragment = url.fragment().and_then(|fragment| {
        let value = fragment
            .strip_prefix("page=")
            .or_else(|| fragment.strip_prefix("page"))
            .unwrap_or(fragment)
            .trim_start_matches(['=', ':']);
        value.parse::<usize>().ok()
    });
    from_query
        .or(from_fragment)
        .map(|page| page.saturating_sub(1))
}

fn internal_pdf_page_target(target: &str) -> Option<usize> {
    target
        .strip_prefix("#page=")
        .or_else(|| target.strip_prefix("#page"))
        .and_then(|page| page.trim_start_matches(['=', ':']).parse::<usize>().ok())
        .map(|page| page.saturating_sub(1))
}

fn source_position_from_url(url: &url::Url) -> Option<(usize, usize)> {
    let mut line = None;
    let mut column = 1;
    for (key, value) in url.query_pairs() {
        if key.eq_ignore_ascii_case("line") {
            line = value.parse::<usize>().ok();
        } else if key.eq_ignore_ascii_case("column") {
            column = value.parse::<usize>().unwrap_or(1);
        }
    }
    if line.is_none()
        && let Some(fragment) = url.fragment()
    {
        let fragment = fragment
            .strip_prefix("line=")
            .or_else(|| fragment.strip_prefix("line"))
            .or_else(|| fragment.strip_prefix('L'))
            .unwrap_or(fragment)
            .trim_start_matches(['=', ':']);
        let mut parts = fragment.split([':', ',']);
        line = parts.next().and_then(|value| value.parse::<usize>().ok());
        column = parts
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(column);
    }
    line.map(|line| (line.max(1), column.max(1)))
}

fn char_index_at_line_column(source: &str, line: usize, column: usize) -> usize {
    let target_line = line.saturating_sub(1);
    let target_column = column.saturating_sub(1);
    let mut absolute = 0;
    for (index, segment) in source.split_inclusive('\n').enumerate() {
        if index == target_line {
            let line = segment.strip_suffix('\n').unwrap_or(segment);
            return absolute + line.chars().count().min(target_column);
        }
        absolute += segment.chars().count();
    }
    source.chars().count()
}

fn open_in_system_browser(target: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(target).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(target)
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = std::process::Command::new("xdg-open").arg(target).spawn();
    result
        .map(|_| ())
        .map_err(|error| format!("Could not open the link in the system browser: {error}"))
}

fn diagnostic_color(severity: DiagnosticSeverity, dark_mode: bool) -> Color32 {
    match severity {
        DiagnosticSeverity::Error => error_color(dark_mode),
        DiagnosticSeverity::Warning => warning_color(dark_mode),
        DiagnosticSeverity::Help | DiagnosticSeverity::Note => info_color(dark_mode),
        DiagnosticSeverity::Unknown => neutral_color(dark_mode),
    }
}

fn error_color(dark_mode: bool) -> Color32 {
    theme::palette(dark_mode).error
}

fn warning_color(dark_mode: bool) -> Color32 {
    theme::palette(dark_mode).warning
}

fn info_color(dark_mode: bool) -> Color32 {
    theme::palette(dark_mode).info
}

fn success_color(dark_mode: bool) -> Color32 {
    theme::palette(dark_mode).success
}

fn neutral_color(dark_mode: bool) -> Color32 {
    theme::palette(dark_mode).neutral
}

fn notice_color(kind: NoticeKind, dark_mode: bool) -> Color32 {
    match kind {
        NoticeKind::Info => info_color(dark_mode),
        NoticeKind::Success => success_color(dark_mode),
        NoticeKind::Error => error_color(dark_mode),
    }
}

fn theme_label(theme: Option<egui::Theme>) -> &'static str {
    match theme {
        Some(egui::Theme::Light) => "Light",
        Some(egui::Theme::Dark) => "Dark",
        None => "Unavailable",
    }
}

fn color_theme_choice_label(choice: &ColorThemeChoice) -> String {
    match choice {
        ColorThemeChoice::Builtin(id) => builtin_themes::find(id)
            .map_or_else(|| format!("Unknown · {id}"), |theme| theme.name.to_owned()),
        ColorThemeChoice::Sublime(path) => Path::new(path)
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map_or_else(
                || "Imported Sublime theme".to_owned(),
                |name| name.to_owned(),
            ),
    }
}

fn show_typst_override_editor(
    ui: &mut egui::Ui,
    overrides: &mut TypstStyleOverrides,
    palette: theme::SyntaxPalette,
    syntect_theme: &syntect::highlighting::Theme,
) {
    for group in ["Markup", "Math", "Code", "Diagnostics"] {
        ui.label(RichText::new(group).strong());
        egui::Grid::new(("typst-override-grid", group))
            .num_columns(9)
            .striped(true)
            .spacing(egui::vec2(theme::SPACE.control, theme::SPACE.tight))
            .show(ui, |ui| {
                ui.add_sized(
                    [METRICS.settings.override_role_width, 0.0],
                    egui::Label::new(RichText::new("Syntax").small().weak()),
                );
                for label in [
                    "Foreground",
                    "Background",
                    "Bold",
                    "Italic",
                    "Underline",
                    "Strike",
                ] {
                    ui.label(RichText::new(label).small().weak());
                }
                ui.add_sized(
                    [METRICS.settings.override_sample_width, 0.0],
                    egui::Label::new(RichText::new("Live sample").small().weak()),
                );
                ui.end_row();

                for role in TypstSyntaxRole::ALL
                    .into_iter()
                    .filter(|role| role.group() == group)
                {
                    let inherited = ResolvedTypstStyles::resolve_style(
                        role,
                        palette,
                        Some(syntect_theme),
                        None,
                    );
                    let mut style_override = overrides.get(role).cloned().unwrap_or_default();
                    typst_overrides_hover_text(
                        ui.add_sized(
                            [METRICS.settings.override_role_width, 0.0],
                            egui::Label::new(role.label()).truncate(),
                        ),
                        role.label(),
                    );
                    optional_color_override(
                        ui,
                        (role, "foreground"),
                        &mut style_override.foreground,
                        inherited.foreground,
                        "foreground",
                    );
                    optional_color_override(
                        ui,
                        (role, "background"),
                        &mut style_override.background,
                        inherited.background,
                        "background",
                    );
                    optional_bool_override(ui, (role, "bold"), "B", &mut style_override.bold);
                    optional_bool_override(ui, (role, "italic"), "I", &mut style_override.italic);
                    optional_bool_override(
                        ui,
                        (role, "underline"),
                        "U",
                        &mut style_override.underline,
                    );
                    optional_bool_override(
                        ui,
                        (role, "strike"),
                        "S",
                        &mut style_override.strikethrough,
                    );

                    let resolved = ResolvedTypstStyles::resolve_style(
                        role,
                        palette,
                        Some(syntect_theme),
                        Some(&style_override),
                    );
                    let mut sample = egui::text::LayoutJob::default();
                    sample.append(role.sample(), 0.0, resolved.text_format());
                    typst_overrides_hover_text(
                        ui.add_sized(
                            [METRICS.settings.override_sample_width, 0.0],
                            egui::Label::new(sample).truncate(),
                        ),
                        role.sample(),
                    );

                    if ui
                        .add_sized(
                            [METRICS.settings.override_reset_width, 0.0],
                            egui::Button::new("Reset"),
                        )
                        .clicked()
                    {
                        style_override = TypstStyleOverride::default();
                    }
                    overrides.set(role, style_override);
                    ui.end_row();
                }
            });
        ui.add_space(theme::SPACE.small);
    }
}

fn optional_color_override(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut Option<Rgba>,
    inherited: Color32,
    field: &str,
) {
    ui.push_id(id, |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(
                METRICS.settings.override_color_width,
                METRICS.icon.button_size.y,
            ),
            Layout::left_to_right(Align::Center),
            |ui| {
                theme::apply_compact_control_spacing(ui);
                let enabled = value.is_some();
                if typst_overrides_hover_text(
                    ui.selectable_label(enabled, if enabled { "set" } else { "theme" }),
                    if enabled {
                        format!("{field} overrides the selected theme; click to inherit")
                    } else {
                        format!("{field} inherits the selected theme; click to override")
                    },
                )
                .clicked()
                {
                    *value = if enabled {
                        None
                    } else {
                        Some(rgba_from_color(inherited))
                    };
                }
                let mut color = value.map_or(inherited, color_from_rgba);
                ui.add_enabled_ui(value.is_some(), |ui| {
                    if ui.color_edit_button_srgba(&mut color).changed() {
                        *value = Some(rgba_from_color(color));
                    }
                });
            },
        );
    });
}

fn optional_bool_override(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    value: &mut Option<bool>,
) {
    let state = match value {
        None => "-",
        Some(true) => "on",
        Some(false) => "off",
    };
    if ui
        .push_id(id, |ui| {
            typst_overrides_hover_text(
                ui.add_sized(
                    [
                        METRICS.settings.override_decoration_width,
                        METRICS.icon.button_size.y,
                    ],
                    egui::Button::new(format!("{label} {state}")),
                ),
                "Click to cycle: inherit, on, off",
            )
        })
        .inner
        .clicked()
    {
        *value = match *value {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
    }
}

fn rgba_from_color(color: Color32) -> Rgba {
    Rgba::from_rgba(color.r(), color.g(), color.b(), color.a())
}

fn color_from_rgba(color: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn settings_value_row(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        settings_inline_value(ui, name, value);
    });
}

fn settings_inline_value(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.label(RichText::new(format!("{name}:")).weak());
    ui.label(value);
}

fn tool_preference_editor(
    ui: &mut egui::Ui,
    label: &str,
    preference: &mut ToolPreference,
    resolution: &ToolResolution,
    deterministic_snapshot: bool,
) -> bool {
    let mut browse = false;
    ui.push_id(label, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(label).strong());
            for mode in ToolMode::ALL {
                ui.selectable_value(&mut preference.mode, mode, mode.label());
            }
            ui.separator();
            let (origin_label, color, full_path) = if deterministic_snapshot {
                (
                    "Bundled",
                    success_color(ui.visuals().dark_mode),
                    format!(
                        "<packaged>/{} {}",
                        resolution.kind.binary_name(),
                        resolution.kind.bundled_version()
                    ),
                )
            } else {
                let color = match resolution.origin {
                    ToolOrigin::Bundled | ToolOrigin::Custom => {
                        success_color(ui.visuals().dark_mode)
                    }
                    ToolOrigin::Environment | ToolOrigin::Path => {
                        warning_color(ui.visuals().dark_mode)
                    }
                    ToolOrigin::Missing => error_color(ui.visuals().dark_mode),
                };
                (
                    resolution.origin.label(),
                    color,
                    resolution.program.display().to_string(),
                )
            };
            ui.label(RichText::new(origin_label).small().strong().color(color));
            let path_width = ui
                .available_width()
                .max(METRICS.settings.tool_path_min_width);
            let path_chars = approximate_char_capacity(
                path_width,
                METRICS.settings.tool_path_estimated_font_size,
            );
            let path = ui.add_sized(
                [path_width, METRICS.settings.tool_path_row_height],
                egui::Label::new(
                    RichText::new(tail_elide(&full_path, path_chars))
                        .small()
                        .monospace(),
                )
                .truncate(),
            );
            native_hover_text(path, full_path);
        });
        if preference.mode == ToolMode::Custom {
            ui.horizontal_wrapped(|ui| {
                let path_width =
                    (ui.available_width() - METRICS.settings.tool_custom_label_reserve).clamp(
                        METRICS.settings.tool_custom_min_width,
                        METRICS.settings.tool_custom_max_width,
                    );
                ui.add(
                    egui::TextEdit::singleline(&mut preference.custom_path)
                        .hint_text("/absolute/path/to/executable")
                        .desired_width(path_width),
                );
                browse |= ui.button("Browse…").clicked();
            });
        }
    });
    browse
}

fn fallback_notice(ui: &mut egui::Ui, label: &str, reason: &str) {
    let color = warning_color(ui.visuals().dark_mode);
    ui.group(|ui| {
        ui.label(RichText::new(label).small().strong().color(color));
        ui.label(reason);
    });
}

fn show_tool_status_chip(ui: &mut egui::Ui, name: &str, resolution: &ToolResolution) {
    let color = match resolution.origin {
        ToolOrigin::Bundled | ToolOrigin::Custom => success_color(ui.visuals().dark_mode),
        ToolOrigin::Environment | ToolOrigin::Path => warning_color(ui.visuals().dark_mode),
        ToolOrigin::Missing => error_color(ui.visuals().dark_mode),
    };
    show_status_chip(
        ui,
        name,
        resolution.origin.label(),
        &resolution.detail(),
        color,
    );
}

fn show_service_status_chip(ui: &mut egui::Ui, name: &str, state: &ServiceState) {
    let color = match state {
        ServiceState::Ready(_) => success_color(ui.visuals().dark_mode),
        ServiceState::Starting(_) => info_color(ui.visuals().dark_mode),
        ServiceState::Degraded(_) => warning_color(ui.visuals().dark_mode),
        ServiceState::Failed(_) => error_color(ui.visuals().dark_mode),
        ServiceState::Disabled(_) | ServiceState::Unsupported(_) => {
            neutral_color(ui.visuals().dark_mode)
        }
    };
    show_status_chip(ui, name, state.label(), state.detail(), color);
}

fn show_status_chip(ui: &mut egui::Ui, name: &str, status: &str, detail: &str, color: Color32) {
    let response = theme::status_chip_frame(ui.style())
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(name).strong());
                ui.label(RichText::new(status).small().strong().color(color));
            });
        })
        .response;
    settings_hover_text(response, detail);
}

fn preview_fallback_reason_for(
    preference: PreviewPreference,
    interactive_active: bool,
    preview_url_available: bool,
    tinymist_state: &ServiceState,
    webview_state: &ServiceState,
) -> Option<String> {
    if preference != PreviewPreference::Interactive || interactive_active {
        return None;
    }
    let state = if preview_url_available {
        webview_state
    } else {
        tinymist_state
    };
    Some(format!("{}: {}", state.label(), state.detail()))
}

fn preview_backend_label_for(
    preference: PreviewPreference,
    interactive_active: bool,
) -> &'static str {
    if interactive_active {
        "Interactive"
    } else if preference == PreviewPreference::Interactive {
        "Rasterised PDF · fallback"
    } else {
        "Rasterised PDF"
    }
}

fn non_preview_fallback_details_for(
    settings: &AppSettings,
    system_theme: Option<egui::Theme>,
    typst: &ToolResolution,
    tinymist: &ToolResolution,
) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(reason) = settings.interface_theme.fallback_reason(system_theme) {
        details.push(format!("Appearance: {reason}"));
    }
    if let Some(reason) = &typst.fallback_reason {
        details.push(format!("Typst: {reason}"));
    }
    if let Some(reason) = &tinymist.fallback_reason {
        details.push(format!("Tinymist: {reason}"));
    }
    details
}

fn tinymist_diagnostic(diagnostic: TinymistDiagnostic, source: DiagnosticSource) -> Diagnostic {
    let severity = match diagnostic.severity {
        Some(TinymistDiagnosticSeverity::Error) => DiagnosticSeverity::Error,
        Some(TinymistDiagnosticSeverity::Warning) => DiagnosticSeverity::Warning,
        Some(TinymistDiagnosticSeverity::Information) => DiagnosticSeverity::Note,
        Some(TinymistDiagnosticSeverity::Hint) => DiagnosticSeverity::Help,
        Some(TinymistDiagnosticSeverity::Other(_)) | None => DiagnosticSeverity::Unknown,
    };
    let mut details = Vec::new();
    // The editor is Typst-specific, so repeating Tinymist's provider name in
    // every diagnostic is noise.
    let _provider = diagnostic.source;
    if let Some(code) = diagnostic.code {
        details.push(format!("code: {code}"));
    }
    Diagnostic {
        severity,
        source,
        location: Some(DiagnosticLocation {
            line: diagnostic.range.start.line as usize + 1,
            column: diagnostic.range.start.character as usize + 1,
        }),
        message: diagnostic.message,
        details,
    }
}

fn severity_rank(severity: DiagnosticSeverity) -> u8 {
    match severity {
        DiagnosticSeverity::Error => 0,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Help => 2,
        DiagnosticSeverity::Note => 3,
        DiagnosticSeverity::Unknown => 4,
    }
}

fn revision_as_i32(revision: u64) -> i32 {
    revision.min(i32::MAX as u64) as i32
}

fn approximate_char_capacity(width: f32, font_size: f32) -> usize {
    (width.max(0.0) / (font_size * METRICS.text.approximate_character_ratio).max(1.0)).floor()
        as usize
}

fn tail_elide(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars == 1 {
        return "…".to_owned();
    }

    let tail = text
        .chars()
        .rev()
        .take(max_chars - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("…{tail}")
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn viewport_scoped_id(context: &egui::Context, salt: &'static str) -> egui::Id {
    egui::Id::new((context.viewport_id(), salt))
}

fn scoped_child_viewport_id(context: &egui::Context, salt: &'static str) -> egui::ViewportId {
    egui::ViewportId::from_hash_of((context.viewport_id(), salt))
}

fn source_editor_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-source-editor")
}

fn native_hover_tooltip_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-native-hover-tooltip")
}

fn tooltip_geometry_id(context: &egui::Context) -> egui::Id {
    native_hover_tooltip_id(context).with("geometry")
}

fn settings_hover_tooltip_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-settings-hover-tooltip")
}

fn typst_overrides_hover_tooltip_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-typst-overrides-hover-tooltip")
}

fn hover_runtime_config_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-hover-runtime-config")
}

/// Install the per-viewport hover timing without re-entering egui's context
/// lock. `viewport_id()` itself reads context state, so the ID must be derived
/// before `data_mut` takes the write lock. This runs during every first frame.
fn install_hover_runtime_config(context: &egui::Context, delay: Duration, fade: Duration) {
    let id = hover_runtime_config_id(context);
    let config = HoverRuntimeConfig { delay, fade };
    context.data_mut(|data| data.insert_temp(id, config));
}

fn diagnostic_hover_timing_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-diagnostic-hover-timing")
}

fn native_hover_text(response: egui::Response, detail: impl Into<String>) -> egui::Response {
    let id = native_hover_tooltip_id(&response.ctx);
    hover_text_with_id(response, detail, id)
}

fn settings_hover_text(response: egui::Response, detail: impl Into<String>) -> egui::Response {
    let id = settings_hover_tooltip_id(&response.ctx);
    hover_text_with_id(response, detail, id)
}

fn typst_overrides_hover_text(
    response: egui::Response,
    detail: impl Into<String>,
) -> egui::Response {
    let id = typst_overrides_hover_tooltip_id(&response.ctx);
    hover_text_with_id(response, detail, id)
}

fn hover_text_with_id(
    response: egui::Response,
    detail: impl Into<String>,
    id: egui::Id,
) -> egui::Response {
    if let Some(opacity) = hover_opacity(&response, id.with("timing")) {
        let tooltip = HoverTooltipOverlay {
            origin: response.rect,
            anchor: response.rect.left_bottom() + egui::vec2(0.0, theme::SPACE.small),
            detail: detail.into(),
            opacity,
        };
        response.ctx.data_mut(|data| data.insert_temp(id, tooltip));
    }
    response
}

fn hover_opacity(response: &egui::Response, timing_id: egui::Id) -> Option<f32> {
    if !response.hovered() {
        return None;
    }
    let now = response.ctx.input(|input| input.time);
    let runtime_config_id = hover_runtime_config_id(&response.ctx);
    let config = response.ctx.data(|data| {
        data.get_temp::<HoverRuntimeConfig>(runtime_config_id)
            .unwrap_or(HoverRuntimeConfig {
                delay: Duration::from_millis(DEFAULT_HOVER_DELAY_MS),
                fade: Duration::from_millis(DEFAULT_HOVER_FADE_MS),
            })
    });
    let mut state = response.ctx.data(|data| {
        data.get_temp::<HoverTimingState>(timing_id)
            .unwrap_or(HoverTimingState {
                widget: response.id,
                started: now,
                last_seen: now,
            })
    });
    // A missed frame means the pointer left this widget; begin a fresh wait
    // instead of flashing a previously armed tooltip back immediately.
    if state.widget != response.id
        || Duration::from_secs_f64((now - state.last_seen).max(0.0))
            > METRICS.motion.hover_reset_gap
    {
        state = HoverTimingState {
            widget: response.id,
            started: now,
            last_seen: now,
        };
    } else {
        state.last_seen = now;
    }
    response
        .ctx
        .data_mut(|data| data.insert_temp(timing_id, state));

    let elapsed = Duration::from_secs_f64((now - state.started).max(0.0));
    if elapsed < config.delay {
        response
            .ctx
            .request_repaint_after((config.delay - elapsed).min(METRICS.motion.hover_poll));
        return None;
    }
    let fade_elapsed = elapsed.saturating_sub(config.delay);
    let opacity = if config.fade.is_zero() {
        1.0
    } else {
        (fade_elapsed.as_secs_f32() / config.fade.as_secs_f32()).clamp(0.0, 1.0)
    };
    if opacity < 1.0 {
        response
            .ctx
            .request_repaint_after(METRICS.motion.animation_frame);
    }
    Some(opacity)
}

#[allow(clippy::too_many_arguments)]
fn show_native_tooltip_card(
    context: &egui::Context,
    viewport_salt: &'static str,
    anchor: Pos2,
    origin: Rect,
    detail: &str,
    severity: Option<DiagnosticSeverity>,
    opacity: f32,
    captures: &CaptureController,
) {
    let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
        return;
    };
    let card_margin = theme::SPACE.content;
    let theme = context.theme();
    let style = context.style_of(theme);
    let body_font = egui::TextStyle::Body.resolve(&style);
    let desired_card_width = if severity.is_some() {
        METRICS.popup.tooltip_width
    } else {
        let natural_width = context.fonts_mut(|fonts| {
            fonts
                .layout(
                    detail.to_owned(),
                    body_font.clone(),
                    style.visuals.text_color(),
                    f32::INFINITY,
                )
                .size()
                .x
        });
        (natural_width + METRICS.popup.tooltip_text_padding).clamp(
            METRICS.popup.tooltip_min_width,
            METRICS.popup.tooltip_max_width,
        )
    };
    let available_width = (window_rect.width() - METRICS.popup.viewport_edge * 2.0).max(1.0);
    let width = (desired_card_width + card_margin * 2.0).min(available_width);
    let card_width = (width - card_margin * 2.0).max(1.0);
    let body_height = context.fonts_mut(|fonts| {
        fonts
            .layout(
                detail.to_owned(),
                body_font,
                style.visuals.text_color(),
                (card_width - METRICS.popup.tooltip_text_padding).max(1.0),
            )
            .size()
            .y
    });
    let available_height = (window_rect.height() - METRICS.popup.viewport_edge * 2.0).max(1.0);
    let height = (METRICS.popup.tooltip_title_height + body_height + card_margin * 2.0)
        .clamp(
            METRICS.popup.tooltip_min_height,
            METRICS.popup.tooltip_max_height,
        )
        .min(available_height);
    let desired = window_rect.min + anchor.to_vec2();
    let position = Pos2::new(
        desired.x.clamp(
            window_rect.left() + METRICS.popup.viewport_edge,
            window_rect.right() - width - METRICS.popup.viewport_edge,
        ),
        desired.y.clamp(
            window_rect.top() + METRICS.popup.viewport_edge,
            window_rect.bottom() - height - METRICS.popup.viewport_edge,
        ),
    );
    let geometry_id = tooltip_geometry_id(context);
    context.data_mut(|data| {
        data.insert_temp(
            geometry_id,
            TooltipGeometry {
                origin,
                card: Rect::from_min_size(position, Vec2::new(width, height)),
                pointer_inside_card: false,
            },
        );
    });
    let native_theme = theme::native_theme(theme);
    let capture_viewport = captures.has_pending_for("diagnostic");

    context.show_viewport_immediate(
        scoped_child_viewport_id(context, viewport_salt),
        theme::popup_viewport_builder("tiptoptyp")
            .with_position(position)
            .with_inner_size([width, height])
            .with_min_inner_size([width, height])
            .with_max_inner_size([width, height])
            // Keep the popup non-activating in production, while still letting
            // it receive pointer movement and wheel events for scrolling. A
            // queued QA capture temporarily activates its isolated viewport so
            // macOS supplies the repeated paint passes needed by the settling
            // countdown and framebuffer callback.
            .with_active(capture_viewport)
            .with_mouse_passthrough(false),
        |ui, _class| {
            let pointer_inside_card = ui.rect_contains_pointer(ui.max_rect());
            context.data_mut(|data| {
                if let Some(mut geometry) = data.get_temp::<TooltipGeometry>(geometry_id) {
                    geometry.pointer_inside_card = pointer_inside_card;
                    data.insert_temp(geometry_id, geometry);
                }
            });
            captures.begin_viewport(ui.ctx());
            ui.set_style(style.clone());
            ui.set_opacity(opacity);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
            theme::tooltip_card_frame(&style).show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| show_markdown(ui, detail));
            });
            captures.end_glow_viewport(ui, "diagnostic");
        },
    );
}

fn show_local_tooltip_card(context: &egui::Context, anchor: Pos2, detail: &str, opacity: f32) {
    let style = context.style_of(context.theme());
    egui::Area::new(viewport_scoped_id(context, "settings-tooltip-card"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor)
        .constrain_to(context.content_rect())
        .show(context, |ui| {
            ui.set_opacity(opacity);
            theme::tooltip_card_frame(&style).show(ui, |ui| {
                ui.set_max_width(METRICS.popup.tooltip_max_width);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(METRICS.popup.tooltip_max_height)
                    .show(ui, |ui| show_markdown(ui, detail));
            });
        });
}

fn show_markdown(ui: &mut egui::Ui, markdown: &str) {
    let mut fenced = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            ui.monospace(line);
            continue;
        }
        if trimmed.is_empty() {
            ui.add_space(theme::SPACE.small);
            continue;
        }

        let (prefix, content, heading) = if let Some(content) = trimmed.strip_prefix("### ") {
            ("", content, 1.05)
        } else if let Some(content) = trimmed.strip_prefix("## ") {
            ("", content, 1.1)
        } else if let Some(content) = trimmed.strip_prefix("# ") {
            ("", content, 1.15)
        } else if let Some(content) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            ("• ", content, 1.0)
        } else {
            ("", trimmed, 1.0)
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if !prefix.is_empty() {
                ui.label(prefix);
            }
            show_markdown_inline(ui, content, heading);
        });
    }
}

fn show_markdown_inline(ui: &mut egui::Ui, text: &str, scale: f32) {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut code = false;
    let mut bold = false;
    let mut italics = false;
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        let (kind, marker, marker_len) = if chars[index] == '`' {
            (Some(0), '`', 1)
        } else if chars[index] == '*' && chars.get(index + 1) == Some(&'*') {
            (Some(1), '*', 2)
        } else if chars[index] == '_' && chars.get(index + 1) == Some(&'_') {
            (Some(1), '_', 2)
        } else if chars[index] == '*' {
            (Some(2), '*', 1)
        } else if chars[index] == '_' {
            (Some(2), '_', 1)
        } else {
            (None, '\0', 0)
        };
        if let Some(kind) = kind
            && marker_is_closed(&chars, index + marker_len, marker, marker_len)
        {
            if !current.is_empty() {
                spans.push((std::mem::take(&mut current), code, bold, italics));
            }
            match kind {
                0 => code = !code,
                1 => bold = !bold,
                _ => italics = !italics,
            }
            index += marker_len;
        } else {
            current.push(chars[index]);
            index += 1;
        }
    }
    if !current.is_empty() {
        spans.push((current, code, bold, italics));
    }
    for (text, code, bold, italics) in spans {
        let mut rich = RichText::new(text);
        if code {
            rich = rich.monospace();
        }
        if bold {
            rich = rich.strong();
        }
        if italics {
            rich = rich.italics();
        }
        if scale != 1.0 {
            rich = rich.size(theme::TYPE.content * scale);
        }
        ui.label(rich);
    }
}

fn marker_is_closed(chars: &[char], start: usize, marker: char, length: usize) -> bool {
    chars.get(start..).is_some_and(|rest| {
        rest.windows(length)
            .any(|window| window.iter().all(|character| *character == marker))
    })
}

fn tooltip_region_contains(pointer: Pos2, origin: Rect, card: Rect) -> bool {
    if origin.contains(pointer) || card.contains(pointer) {
        return true;
    }
    let (a, b, c) = if card.left() >= origin.right() {
        (
            Pos2::new(origin.right(), origin.top()),
            Pos2::new(origin.right(), origin.bottom()),
            Pos2::new(card.left(), card.center().y),
        )
    } else if card.right() <= origin.left() {
        (
            Pos2::new(origin.left(), origin.top()),
            Pos2::new(origin.left(), origin.bottom()),
            Pos2::new(card.right(), card.center().y),
        )
    } else if card.top() >= origin.bottom() {
        (
            Pos2::new(origin.left(), origin.bottom()),
            Pos2::new(origin.right(), origin.bottom()),
            Pos2::new(card.center().x, card.top()),
        )
    } else {
        (
            Pos2::new(origin.left(), origin.top()),
            Pos2::new(origin.right(), origin.top()),
            Pos2::new(card.center().x, card.bottom()),
        )
    };
    let ab = b - a;
    let ac = c - a;
    let ap = pointer - a;
    let denominator = ab.x * ac.y - ab.y * ac.x;
    if denominator.abs() < f32::EPSILON {
        return false;
    }
    let u = (ap.x * ac.y - ap.y * ac.x) / denominator;
    let v = (ab.x * ap.y - ab.y * ap.x) / denominator;
    u >= 0.0 && v >= 0.0 && u + v <= 1.0
}

fn clamp_cursor_range(range: CCursorRange, len: usize) -> CCursorRange {
    CCursorRange {
        primary: CCursor::new(range.primary.index.0.min(len)),
        secondary: CCursor::new(range.secondary.index.0.min(len)),
        h_pos: range.h_pos,
    }
}

fn menu_button<'a>(
    ui: &egui::Ui,
    label: &'a str,
    shortcut: Option<KeyboardShortcut>,
) -> egui::Button<'a> {
    match shortcut {
        Some(shortcut) => egui::Button::new(label)
            .shortcut_text(ui.ctx().format_shortcut(&shortcut))
            .frame(false),
        None => egui::Button::new(label).frame(false),
    }
}

fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: Option<KeyboardShortcut>) -> egui::Response {
    let width = ui.available_width().max(1.0);
    ui.add_sized(
        [width, METRICS.menu.row_height],
        menu_button(ui, label, shortcut),
    )
}

fn menu_item_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    shortcut: Option<KeyboardShortcut>,
) -> egui::Response {
    let width = ui.available_width().max(1.0);
    ui.add_enabled_ui(enabled, |ui| {
        ui.add_sized(
            [width, METRICS.menu.row_height],
            menu_button(ui, label, shortcut),
        )
    })
    .inner
}

fn show_file_popup_ui(ui: &mut egui::Ui, action: &mut Option<AppPopupAction>) {
    let entries = [
        (
            "New",
            KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::N),
            FileMenuAction::New,
        ),
        (
            "New Window",
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::N),
            FileMenuAction::NewWindow,
        ),
        (
            "Open…",
            KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::O),
            FileMenuAction::Open,
        ),
        (
            "Open in New Window…",
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, egui::Key::O),
            FileMenuAction::OpenInNewWindow,
        ),
        (
            "Change Workspace Root…",
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::O),
            FileMenuAction::ChangeWorkspaceRoot,
        ),
        (
            "Save",
            KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::S),
            FileMenuAction::Save,
        ),
        (
            "Save As…",
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::S),
            FileMenuAction::SaveAs,
        ),
    ];
    for (label, shortcut, file_action) in entries {
        if menu_item(ui, label, Some(shortcut)).clicked() {
            *action = Some(AppPopupAction::File(file_action));
        }
    }
    ui.separator();
    if menu_item(
        ui,
        "Export PDF…",
        Some(KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            egui::Key::E,
        )),
    )
    .clicked()
    {
        *action = Some(AppPopupAction::File(FileMenuAction::ExportPdf));
    }
}

fn show_edit_popup_ui(
    ui: &mut egui::Ui,
    can_undo: bool,
    can_redo: bool,
    can_format: bool,
    action: &mut Option<AppPopupAction>,
) {
    if menu_item_enabled(
        ui,
        can_undo,
        "Undo",
        Some(KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Z)),
    )
    .clicked()
    {
        *action = Some(AppPopupAction::Editor(EditorMenuAction::Undo));
    }
    if menu_item_enabled(
        ui,
        can_redo,
        "Redo",
        Some(KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            egui::Key::Z,
        )),
    )
    .clicked()
    {
        *action = Some(AppPopupAction::Editor(EditorMenuAction::Redo));
    }
    ui.separator();
    for (label, key, editor_action) in [
        ("Cut", egui::Key::X, EditorMenuAction::Cut),
        ("Copy", egui::Key::C, EditorMenuAction::Copy),
        ("Paste", egui::Key::V, EditorMenuAction::Paste),
        ("Select All", egui::Key::A, EditorMenuAction::SelectAll),
    ] {
        if menu_item(
            ui,
            label,
            Some(KeyboardShortcut::new(Modifiers::COMMAND, key)),
        )
        .clicked()
        {
            *action = Some(AppPopupAction::Editor(editor_action));
        }
    }
    ui.separator();
    if menu_item(
        ui,
        "Find…",
        Some(KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::F)),
    )
    .clicked()
    {
        *action = Some(AppPopupAction::Find(false));
    }
    if menu_item(ui, "Find and Replace…", Some(replace_shortcut())).clicked() {
        *action = Some(AppPopupAction::Find(true));
    }
    ui.separator();
    if menu_item_enabled(ui, can_format, "Format Document", Some(format_shortcut())).clicked() {
        *action = Some(AppPopupAction::Editor(EditorMenuAction::Format));
    }
}

fn show_workspace_popup_ui(
    ui: &mut egui::Ui,
    path: &Path,
    is_file: bool,
    action: &mut Option<AppPopupAction>,
) {
    if menu_item_enabled(ui, is_file, "Open", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Open(
            path.to_path_buf(),
        )));
    }
    let is_typst = is_file
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("typ"));
    if menu_item_enabled(ui, is_typst, "Use for preview", None).clicked() {
        *action = Some(AppPopupAction::Workspace(
            WorkspaceMenuAction::UseForPreview(path.to_path_buf()),
        ));
    }
    if menu_item_enabled(ui, is_file, "Rename…", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Rename(
            path.to_path_buf(),
        )));
    }
    ui.separator();
    if menu_item(ui, "Copy Path", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::CopyPath(
            path.to_path_buf(),
        )));
    }
    if menu_item(ui, reveal_label(), None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Reveal(
            path.to_path_buf(),
        )));
    }
}

fn clamp_popup_anchor(anchor: Pos2, popup_size: Vec2, viewport_size: Vec2) -> Pos2 {
    let margin = METRICS.popup.viewport_edge;
    let max_x = (viewport_size.x - popup_size.x - margin).max(margin);
    let max_y = (viewport_size.y - popup_size.y - margin).max(margin);
    Pos2::new(anchor.x.clamp(margin, max_x), anchor.y.clamp(margin, max_y))
}

fn show_editor_context_menu_ui(
    ui: &mut egui::Ui,
    can_undo: bool,
    can_redo: bool,
    has_selection: bool,
    can_format: bool,
    action: &mut Option<EditorMenuAction>,
) {
    if menu_item_enabled(ui, can_undo, "Undo", None).clicked() {
        *action = Some(EditorMenuAction::Undo);
        ui.close();
    }
    if menu_item_enabled(ui, can_redo, "Redo", None).clicked() {
        *action = Some(EditorMenuAction::Redo);
        ui.close();
    }
    ui.separator();
    if menu_item_enabled(ui, has_selection, "Cut", None).clicked() {
        *action = Some(EditorMenuAction::Cut);
        ui.close();
    }
    if menu_item_enabled(ui, has_selection, "Copy", None).clicked() {
        *action = Some(EditorMenuAction::Copy);
        ui.close();
    }
    if menu_item(ui, "Paste", None).clicked() {
        *action = Some(EditorMenuAction::Paste);
        ui.close();
    }
    if menu_item(ui, "Select All", None).clicked() {
        *action = Some(EditorMenuAction::SelectAll);
        ui.close();
    }
    if can_format {
        ui.separator();
        if menu_item(ui, "Format Document", None).clicked() {
            *action = Some(EditorMenuAction::Format);
            ui.close();
        }
    }
}

fn show_status_log_popup_ui(ui: &mut egui::Ui, entries: &VecDeque<StatusLogEntry>) {
    ui.label(RichText::new("Recent status").strong());
    ui.separator();
    if entries.is_empty() {
        ui.label(RichText::new("No status changes yet").small().weak());
        return;
    }
    for entry in entries {
        let color = notice_color(entry.kind, ui.visuals().dark_mode);
        ui.horizontal_wrapped(|ui| {
            static_icon(
                ui,
                match entry.kind {
                    NoticeKind::Success => UiIcon::Check,
                    NoticeKind::Error => UiIcon::Warning,
                    NoticeKind::Info => UiIcon::Waiting,
                },
                color,
            );
            ui.label(RichText::new(&entry.detail).small().color(color));
        });
    }
}

fn replace_shortcut() -> KeyboardShortcut {
    #[cfg(target_os = "macos")]
    {
        KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, egui::Key::F)
    }
    #[cfg(not(target_os = "macos"))]
    {
        KeyboardShortcut::new(Modifiers::CTRL, egui::Key::H)
    }
}

fn format_shortcut() -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::SHIFT | Modifiers::ALT, egui::Key::F)
}

fn char_range_to_byte_range(source: &str, range: Range<usize>) -> Range<usize> {
    fn byte_at(source: &str, index: usize) -> usize {
        source
            .char_indices()
            .nth(index)
            .map_or(source.len(), |(byte, _)| byte)
    }
    byte_at(source, range.start)..byte_at(source, range.end)
}

fn canonical_or_absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .map(|directory| directory.join(path))
                .unwrap_or_else(|_| path.to_owned())
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InitialWorkspace {
    root: PathBuf,
    document: Option<PathBuf>,
}

/// Resolve a normal launch without presenting UI. Explicit file and folder
/// targets win; otherwise the newest still-existing workspace is restored,
/// with the process working directory as a first-run fallback.
fn resolve_initial_workspace(
    settings: &AppSettings,
    initial_path: Option<&Path>,
    working_directory: &Path,
) -> InitialWorkspace {
    let explicit_file = initial_path.filter(|path| path.is_file());
    let explicit_directory = initial_path.filter(|path| path.is_dir());
    let remembered_root = initial_path
        .is_none()
        .then(|| settings.existing_recent_workspaces().into_iter().next())
        .flatten();

    let root = if let Some(directory) = explicit_directory {
        canonical_or_absolute(directory)
    } else if let Some(parent) = explicit_file.and_then(Path::parent) {
        canonical_or_absolute(&discover_project_root(parent))
    } else if let Some(remembered_root) = remembered_root {
        canonical_or_absolute(&remembered_root)
    } else {
        canonical_or_absolute(&discover_project_root(working_directory))
    };

    let document = explicit_file.map(canonical_or_absolute).or_else(|| {
        (initial_path.is_none() || explicit_directory.is_some())
            .then(|| remembered_document(settings, &root))
            .flatten()
    });

    InitialWorkspace { root, document }
}

fn remembered_document(settings: &AppSettings, requested_root: &Path) -> Option<PathBuf> {
    let root = canonical_or_absolute(requested_root);
    let key = root.to_str()?;
    let stored = PathBuf::from(settings.last_opened_files.get(key)?);
    let path = stored.canonicalize().ok()?;
    (path.is_file() && path.starts_with(&root) && DocumentKind::supports_path(&path))
        .then_some(path)
}

fn trim_file_history(history: &mut BTreeMap<String, String>, keep: &str) {
    const MAX_PROJECT_HISTORY: usize = 64;
    while history.len() > MAX_PROJECT_HISTORY {
        let Some(stale) = history.keys().find(|key| key.as_str() != keep).cloned() else {
            break;
        };
        history.remove(&stale);
    }
}

fn disk_matches_fingerprint(path: &Path, expected: Option<u64>) -> bool {
    let Some(expected) = expected else {
        // Auto-save is deliberately non-interactive. If we do not know which
        // disk contents the buffer came from, overwriting cannot be proven
        // safe, so require an explicit save instead.
        return false;
    };
    fs::read(path)
        .map(|contents| fingerprint(&contents) == expected)
        .unwrap_or(false)
}

fn reveal_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Reveal in Finder"
    }
    #[cfg(target_os = "windows")]
    {
        "Show in Explorer"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "Show in File Manager"
    }
}

fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let status = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .status();

    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("File manager exited with {status}")),
        Err(error) => Err(format!("Could not open the file manager: {error}")),
    }
}

fn atomic_write(project_root: &Path, path: &Path, contents: &[u8]) -> Result<(), String> {
    crate::private_workspace::atomic_write(project_root, path, contents)
        .map_err(|error| format!("Could not replace {}: {error}", path.display()))
}

fn fingerprint(contents: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    hasher.finish()
}

fn discover_project_root(source_dir: &Path) -> PathBuf {
    source_dir
        .ancestors()
        .find(|ancestor| ancestor.join("typst.toml").is_file() || ancestor.join(".git").exists())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| source_dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tinymist::LspPosition;

    #[test]
    fn first_frame_hover_state_installs_without_reentering_the_context_lock() {
        let context = egui::Context::default();
        let delay = Duration::from_millis(275);
        let fade = Duration::from_millis(45);

        context
            .run_ui(egui::RawInput::default(), |ui| {
                // This is the same first-frame operation that once prevented
                // the native window from ever painting. epaint detects a
                // nested context lock as a deadlock, so completing this call
                // is the regression assertion.
                install_hover_runtime_config(ui.ctx(), delay, fade);
            })
            .drop_without_applying_deltas();

        let id = hover_runtime_config_id(&context);
        let installed = context
            .data(|data| data.get_temp::<HoverRuntimeConfig>(id))
            .expect("the first frame should publish hover timing state");
        assert_eq!(installed.delay, delay);
        assert_eq!(installed.fade, fade);
    }

    #[test]
    fn cursor_coordinates_are_one_based_and_unicode_scalar_aware() {
        let source = "a🦀b\nsecond";
        assert_eq!(line_column_at_char(source, 0), (1, 1));
        assert_eq!(line_column_at_char(source, 2), (1, 3));
        assert_eq!(line_column_at_char(source, 4), (2, 1));
        assert_eq!(line_column_at_char(source, usize::MAX), (2, 7));
    }

    #[test]
    fn semantic_hover_tracks_identifiers_and_nearby_call_parentheses() {
        let source = "#text(fill: blue)[hello]";
        assert_eq!(typst_hover_token_range(source, 2), Some(1..5));
        assert_eq!(typst_hover_token_range(source, 5), Some(1..5));
        assert_eq!(typst_hover_token_range(source, 8), Some(6..10));
        assert_eq!(typst_hover_token_range(source, 0), None);
        assert_eq!(typst_hover_token_range("", 0), None);
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn web_navigation_keeps_preview_routes_embedded_but_rejects_other_origins() {
        let base = "http://127.0.0.1:4173/preview/index.html";
        assert!(same_web_origin(
            base,
            "http://127.0.0.1:4173/assets/page.svg"
        ));
        assert!(same_web_origin(base, "http://127.0.0.1:4173/#page=12"));
        assert!(!same_web_origin(base, "https://typst.app/docs"));
        assert!(!same_web_origin(base, "http://127.0.0.1:4174/preview"));
    }

    #[test]
    fn linked_pdf_pages_are_converted_from_one_based_urls() {
        assert_eq!(
            pdf_page_from_url(&url::Url::parse("file:///tmp/paper.pdf#page=7").unwrap()),
            Some(6)
        );
        assert_eq!(
            pdf_page_from_url(&url::Url::parse("file:///tmp/paper.pdf?page=3").unwrap()),
            Some(2)
        );
        assert_eq!(
            pdf_page_from_url(&url::Url::parse("file:///tmp/paper.pdf#section").unwrap()),
            None
        );
        assert_eq!(internal_pdf_page_target("#page=9"), Some(8));
    }

    #[test]
    fn image_texture_rebuild_uses_raster_not_logical_dimensions() {
        let rgba = vec![255; 2 * 3 * 4];
        let image = preview_color_image([2, 3], &rgba);
        assert_eq!(image.size, [2, 3]);
        assert_eq!(image.pixels.len(), 6);
    }

    #[test]
    fn queued_exports_follow_edits_but_never_cross_documents() {
        let mut pending = Some(PendingExport {
            path: PathBuf::from("old-document.pdf"),
            document_epoch: 7,
        });
        assert_eq!(take_matching_export(&mut pending, 8), None);
        assert!(pending.is_none());

        let mut pending = Some(PendingExport {
            path: PathBuf::from("current-document.pdf"),
            document_epoch: 8,
        });
        assert_eq!(
            take_matching_export(&mut pending, 8),
            Some(PathBuf::from("current-document.pdf"))
        );
    }

    #[test]
    fn typst_link_fragments_map_to_unicode_source_positions() {
        let url = url::Url::parse("file:///tmp/chapter.typ#line=2:2").unwrap();
        assert_eq!(source_position_from_url(&url), Some((2, 2)));
        assert_eq!(char_index_at_line_column("one\n🦀two\n", 2, 2), 5);
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn preview_server_document_links_route_back_to_project_files() {
        let project = tempfile::tempdir().unwrap();
        let source_dir = project.path().join("chapters");
        let linked = project.path().join("assets/other paper.pdf");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
        std::fs::write(&linked, b"%PDF-test").unwrap();

        let routed = preview_document_file_url(
            "http://127.0.0.1:4173/assets/other%20paper.pdf#page=4",
            project.path(),
            Some(&source_dir),
        )
        .unwrap();
        let routed = url::Url::parse(&routed).unwrap();
        assert_eq!(
            routed.to_file_path().unwrap(),
            linked.canonicalize().unwrap()
        );
        assert_eq!(routed.fragment(), Some("page=4"));
        assert!(
            preview_document_file_url(
                "http://127.0.0.1:4173/assets/viewer.js",
                project.path(),
                Some(&source_dir),
            )
            .is_none()
        );
    }

    #[test]
    fn atomic_write_replaces_a_file_with_exact_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document.pdf");
        atomic_write(directory.path(), &path, b"first").unwrap();
        atomic_write(directory.path(), &path, b"%PDF-exact\0bytes").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"%PDF-exact\0bytes");
    }

    #[test]
    fn autosave_requires_a_known_disk_fingerprint() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document.typ");
        std::fs::write(&path, "= Original").unwrap();
        assert!(!disk_matches_fingerprint(&path, None));
        assert!(disk_matches_fingerprint(
            &path,
            Some(fingerprint(b"= Original"))
        ));
        assert!(!disk_matches_fingerprint(
            &path,
            Some(fingerprint(b"= Different"))
        ));
    }

    #[test]
    fn project_root_uses_nearest_marker() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        let source = project.join("chapters");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        assert_eq!(discover_project_root(&source), project);
    }

    #[test]
    fn compact_paths_keep_the_tail_and_unicode_character_budget() {
        let path = "/Users/example/Documents/文稿/chapters/introduction.typ";
        let compact = tail_elide(path, 18);
        assert!(compact.starts_with('…'));
        assert!(compact.ends_with("introduction.typ"));
        assert_eq!(compact.chars().count(), 18);
    }

    #[test]
    fn popup_anchor_clamps_inside_even_tiny_viewports() {
        assert_eq!(
            clamp_popup_anchor(
                Pos2::new(900.0, 700.0),
                Vec2::new(280.0, 320.0),
                Vec2::new(220.0, 160.0),
            ),
            Pos2::new(4.0, 4.0)
        );
        assert_eq!(
            clamp_popup_anchor(
                Pos2::new(900.0, 700.0),
                Vec2::new(200.0, 100.0),
                Vec2::new(640.0, 480.0),
            ),
            Pos2::new(436.0, 376.0)
        );
    }

    #[test]
    fn popup_cards_never_paint_dark_corner_shadows() {
        let context = egui::Context::default();
        theme::configure_styles(&context);
        let style = context.style_of(context.theme());
        let frame = theme::popup_card_frame(&style);
        assert_eq!(frame.shadow, egui::epaint::Shadow::NONE);
        assert_eq!(frame.corner_radius, egui::CornerRadius::same(8));
        assert_eq!(style.visuals.popup_shadow, egui::epaint::Shadow::NONE);
        assert_eq!(
            style.visuals.menu_corner_radius,
            egui::CornerRadius::same(8)
        );

        let viewport = theme::popup_viewport_builder("test popup");
        assert_eq!(viewport.transparent, Some(true));
        assert_eq!(viewport.decorations, Some(false));
        assert_eq!(viewport.has_shadow, Some(false));
        assert_eq!(viewport.taskbar, Some(false));
        assert_eq!(
            viewport.window_level,
            Some(egui::viewport::WindowLevel::AlwaysOnTop)
        );
    }

    #[test]
    fn panel_headers_have_exact_and_gapless_geometry_for_varied_controls() {
        let context = egui::Context::default();
        theme::configure_styles(&context);
        let style = context.style_of(context.theme());
        let frame = theme::content_panel_frame(&style);
        assert_eq!(frame.inner_margin.left, 8);
        assert_eq!(frame.inner_margin.right, 8);
        assert_eq!(frame.inner_margin.top, 0);
        assert_eq!(frame.inner_margin.bottom, 0);

        let mut measurements = Vec::new();
        for (id, content) in [
            ("geometry-workspace-header", 0),
            ("geometry-compact-control-header", 1),
            ("geometry-preview-header", 2),
        ] {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 180.0))),
                        ..Default::default()
                    },
                    |ui| {
                        let top = ui.available_rect_before_wrap().top();
                        let mut control_center = 0.0;
                        let header = theme::panel_header(ui, id, |ui| {
                            let response = match content {
                                0 => ui.add_sized([180.0, 20.0], egui::Label::new("…/project")),
                                1 => ui.label(RichText::new("chapter.typ").strong()),
                                _ => ui.selectable_label(true, "Interactive"),
                            };
                            control_center = response.rect.center().y;
                        });
                        measurements.push((
                            top,
                            header,
                            ui.available_rect_before_wrap().top(),
                            control_center,
                        ));
                    },
                )
                .drop_without_applying_deltas();
        }

        for (top, header, body_top, control_center) in &measurements {
            assert!((header.top() - top).abs() < 0.01, "{measurements:?}");
            assert!(
                (header.height() - METRICS.chrome.panel_header_height).abs() < 0.01,
                "{measurements:?}"
            );
            assert!(
                (body_top - header.bottom()).abs() < 0.01,
                "{measurements:?}"
            );
            assert!(
                (control_center - header.center().y).abs() <= 0.51,
                "{measurements:?}"
            );
        }
        assert!(
            measurements.windows(2).all(|pair| {
                (pair[0].1.bottom() - pair[1].1.bottom()).abs() < 0.01
                    && (pair[0].3 - pair[1].3).abs() < 0.01
            }),
            "{measurements:?}"
        );
    }

    #[test]
    fn wide_explorer_header_and_body_cannot_grow_the_resized_panel() {
        let context = egui::Context::default();
        let mut widths = Vec::new();
        let mut header_geometry = Vec::new();
        for _ in 0..4 {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                        ..Default::default()
                    },
                    |ui| {
                        let response = egui::Panel::left("narrow-explorer-test")
                            .default_size(42.0)
                            .min_size(0.0)
                            .max_size(300.0)
                            .show(ui, |ui| {
                                let header =
                                    theme::panel_header(ui, "narrow-explorer-header-test", |ui| {
                                        let _ = ui.add_sized(
                                            [500.0, 60.0],
                                            egui::Label::new("an overflowing explorer header"),
                                        );
                                    });
                                header_geometry
                                    .push((header, ui.available_rect_before_wrap().top()));
                                let mut content =
                                    clipped_panel_content_ui(ui, "narrow-explorer-content-test");
                                content.add_sized(
                                    [500.0, 20.0],
                                    egui::Label::new("a deliberately very wide tree row"),
                                );
                            });
                        widths.push(response.response.rect.width());
                    },
                )
                .drop_without_applying_deltas();
        }
        assert!(widths.iter().all(|width| *width <= 43.0), "{widths:?}");
        assert!(
            header_geometry.iter().all(|(header, body_top)| {
                (header.height() - METRICS.chrome.panel_header_height).abs() < 0.01
                    && (body_top - header.bottom()).abs() < 0.01
            }),
            "{header_geometry:?}"
        );
        assert!(
            header_geometry
                .iter()
                .zip(&widths)
                .all(|((header, _), panel_width)| header.width() <= *panel_width + 0.01),
            "headers={header_geometry:?}, panels={widths:?}"
        );
    }

    #[test]
    fn explorer_sections_split_the_body_budget_without_hiding_headers() {
        let available = 500.0;
        let frame_height = 2.0;
        for open_sections in 1..=EXPLORER_SECTION_SPECS.len() {
            let body =
                available_explorer_section_body_height(available, open_sections, frame_height);
            let headers = EXPLORER_SECTION_SPECS.len() as f32
                * (METRICS.explorer.section_header_height + frame_height);
            let gaps = (EXPLORER_SECTION_SPECS.len() - 1) as f32 * METRICS.explorer.section_gap;
            assert!((headers + gaps + body * open_sections as f32 - available).abs() < 0.01);
        }
        assert_eq!(
            available_explorer_section_body_height(10.0, 2, frame_height),
            0.0
        );
        assert_eq!(
            available_explorer_section_body_height(available, 0, frame_height),
            0.0
        );
    }

    #[test]
    fn explorer_section_bodies_start_at_the_section_root() {
        let context = egui::Context::default();
        theme::configure_styles(&context);
        let mut expected_left = None;
        let mut body_left = None;
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 180.0))),
                    ..Default::default()
                },
                |ui| {
                    expected_left = Some(
                        ui.available_rect_before_wrap().left()
                            + theme::explorer_section_frame(ui.style())
                                .total_margin()
                                .left,
                    );
                    explorer_section(ui, "unindented-section-test", "Files", true, 60.0, |ui| {
                        body_left = Some(ui.available_rect_before_wrap().left())
                    });
                },
            )
            .drop_without_applying_deltas();
        assert_eq!(body_left, expected_left);
    }

    #[test]
    fn explorer_section_frames_fit_the_available_height() {
        let context = egui::Context::default();
        theme::configure_styles(&context);
        let mut used_height = 0.0;
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 500.0))),
                    ..Default::default()
                },
                |ui| {
                    ui.spacing_mut().item_spacing.y = METRICS.explorer.section_gap;
                    let top = ui.available_rect_before_wrap().top();
                    let body_height = explorer_section_body_height(ui);
                    for ((id_salt, default_open), title) in EXPLORER_SECTION_SPECS
                        .into_iter()
                        .zip(["Files", "Contents", "Subfiles", "Symbols", "Packages"])
                    {
                        explorer_section(ui, id_salt, title, default_open, body_height, |_| {});
                    }
                    used_height = ui.min_rect().bottom() - top;
                },
            )
            .drop_without_applying_deltas();
        assert!(used_height <= 500.01, "used {used_height} points");
        assert!(
            used_height >= 490.0,
            "left too much unused space: {used_height}"
        );
    }

    #[test]
    fn view_modes_have_the_requested_panel_matrix() {
        assert!(ViewMode::Code.shows_code());
        assert!(!ViewMode::Code.shows_preview());
        assert!(ViewMode::Split.shows_code());
        assert!(ViewMode::Split.shows_preview());
        assert!(!ViewMode::Preview.shows_code());
        assert!(ViewMode::Preview.shows_preview());
    }

    #[test]
    fn tooltip_bridge_keeps_pointer_transitively_connected_to_the_card() {
        let origin = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0));
        let card = Rect::from_min_max(Pos2::new(30.0, 0.0), Pos2::new(80.0, 30.0));
        assert!(tooltip_region_contains(origin.center(), origin, card));
        assert!(tooltip_region_contains(Pos2::new(20.0, 10.0), origin, card));
        assert!(tooltip_region_contains(Pos2::new(45.0, 15.0), origin, card));
        assert!(!tooltip_region_contains(
            Pos2::new(20.0, 25.0),
            origin,
            card
        ));
    }

    #[test]
    fn remembered_documents_must_stay_inside_the_canonical_project() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        let outside = directory.path().join("outside.typ");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("main.typ"), "= Main").unwrap();
        std::fs::write(&outside, "= Outside").unwrap();

        let root = root.canonicalize().unwrap();
        let mut settings = AppSettings::default();
        settings.last_opened_files.insert(
            root.to_str().unwrap().to_owned(),
            root.join("main.typ").display().to_string(),
        );
        assert_eq!(
            remembered_document(&settings, &root),
            Some(root.join("main.typ").canonicalize().unwrap())
        );

        settings.last_opened_files.insert(
            root.to_str().unwrap().to_owned(),
            outside.display().to_string(),
        );
        assert_eq!(remembered_document(&settings, &root), None);
    }

    #[test]
    fn targetless_launch_restores_the_newest_existing_workspace_and_document() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let fallback = directory.path().join("fallback");
        for root in [&first, &second, &fallback] {
            std::fs::create_dir(root).unwrap();
        }
        std::fs::write(first.join("first.typ"), "= First").unwrap();
        std::fs::write(second.join("second.typ"), "= Second").unwrap();
        let first = first.canonicalize().unwrap();
        let second = second.canonicalize().unwrap();
        let mut settings = AppSettings::default();
        settings.remember_workspace(&first);
        settings.remember_workspace(&second);
        settings.last_opened_files.insert(
            second.to_str().unwrap().to_owned(),
            second.join("second.typ").display().to_string(),
        );

        assert_eq!(
            resolve_initial_workspace(&settings, None, &fallback),
            InitialWorkspace {
                root: second.clone(),
                document: Some(second.join("second.typ").canonicalize().unwrap()),
            }
        );
    }

    #[test]
    fn targetless_launch_skips_missing_recents_then_falls_back_to_the_cwd_project() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        let nested = project.join("chapters");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        let existing = directory.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        let existing = existing.canonicalize().unwrap();

        let mut settings = AppSettings {
            recent_workspaces: vec![
                directory.path().join("missing").display().to_string(),
                existing.display().to_string(),
            ],
            ..AppSettings::default()
        };
        assert_eq!(
            resolve_initial_workspace(&settings, None, &nested).root,
            existing
        );

        settings.recent_workspaces.clear();
        assert_eq!(
            resolve_initial_workspace(&settings, None, &nested).root,
            project.canonicalize().unwrap()
        );
    }

    #[test]
    fn explicit_launch_targets_override_workspace_history() {
        let directory = tempfile::tempdir().unwrap();
        let remembered = directory.path().join("remembered");
        let explicit = directory.path().join("explicit");
        let nested = explicit.join("chapters");
        std::fs::create_dir(&remembered).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(explicit.join(".git")).unwrap();
        let main = explicit.join("main.typ");
        let chapter = nested.join("one.typ");
        std::fs::write(&main, "= Main").unwrap();
        std::fs::write(&chapter, "= One").unwrap();
        let remembered = remembered.canonicalize().unwrap();
        let explicit = explicit.canonicalize().unwrap();
        let mut settings = AppSettings::default();
        settings.remember_workspace(&remembered);
        settings.last_opened_files.insert(
            explicit.to_str().unwrap().to_owned(),
            main.display().to_string(),
        );

        assert_eq!(
            resolve_initial_workspace(&settings, Some(&explicit), directory.path()),
            InitialWorkspace {
                root: explicit.clone(),
                document: Some(main.canonicalize().unwrap()),
            }
        );
        assert_eq!(
            resolve_initial_workspace(&settings, Some(&chapter), directory.path()),
            InitialWorkspace {
                root: explicit,
                document: Some(chapter.canonicalize().unwrap()),
            }
        );
    }

    #[test]
    fn revision_versions_saturate_for_lsp() {
        assert_eq!(revision_as_i32(42), 42);
        assert_eq!(revision_as_i32(u64::MAX), i32::MAX);
    }

    #[test]
    fn logical_line_count_preserves_empty_and_trailing_lines() {
        assert_eq!(logical_line_count(""), 1);
        assert_eq!(logical_line_count("a\n"), 2);
        assert_eq!(logical_line_count("a\n\n🙂"), 3);
        assert_eq!(line_index_at_char("first\nsecond", 5), 0);
        assert_eq!(line_index_at_char("first\nsecond", 6), 1);
    }

    #[test]
    fn editor_attention_progress_is_short_and_monotonic() {
        assert_eq!(editor_attention_progress(Duration::ZERO), 0.0);
        assert!(editor_attention_progress(Duration::from_millis(210)) > 0.45);
        assert_eq!(
            editor_attention_progress(METRICS.motion.editor_attention),
            1.0
        );
    }

    #[test]
    fn short_editor_surface_fills_tall_and_narrow_viewports() {
        for viewport in [
            Rect::from_min_size(Pos2::new(12.0, 18.0), Vec2::new(640.0, 900.0)),
            Rect::from_min_size(Pos2::new(3.0, 7.0), Vec2::new(42.0, 511.0)),
        ] {
            let short_document =
                Rect::from_min_size(viewport.min, Vec2::new(viewport.width(), 84.0));
            let surface = editor_surface_rect(short_document, viewport);

            assert_eq!(surface.left(), viewport.left());
            assert_eq!(surface.right(), viewport.right());
            assert_eq!(surface.top(), viewport.top());
            assert_eq!(surface.bottom(), viewport.bottom());
        }
    }

    #[test]
    fn wrapped_visual_rows_map_back_to_logical_lines() {
        assert_eq!(
            logical_line_row_ranges_from_breaks([false, false, true, false, true, false]),
            vec![0..3, 3..5, 5..6]
        );
        assert_eq!(logical_line_row_ranges_from_breaks([false]), vec![0..1]);
        assert!(logical_line_row_ranges_from_breaks([]).is_empty());
    }

    #[test]
    fn text_edit_wrap_keeps_the_exact_unicode_source_mapping() {
        fn layout(wrap: bool) -> (usize, String, usize) {
            let context = egui::Context::default();
            theme::configure_editor_fonts(&context);
            let mut source =
                "A deliberately long Typst markup line with 🦀 unicode that must wrap cleanly.\n#let x = 1"
                    .to_owned();
            let source_chars = source.chars().count();
            let mut rows = 0;
            let mut laid_out = String::new();
            let mut end = 0;
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(180.0, 300.0))),
                        ..Default::default()
                    },
                    |ui| {
                        ui.set_max_width(180.0);
                        let mut layouter =
                            |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                                let mut job = egui::text::LayoutJob::simple(
                                    buffer.as_str().to_owned(),
                                    theme::editor_font(),
                                    Color32::WHITE,
                                    if wrap { wrap_width } else { f32::INFINITY },
                                );
                                job.break_on_newline = true;
                                ui.fonts_mut(|fonts| fonts.layout_job(job))
                            };
                        let output = egui::TextEdit::multiline(&mut source)
                            .desired_width(180.0)
                            .layouter(&mut layouter)
                            .show(ui);
                        rows = output.galley.rows.len();
                        laid_out = output.galley.text().to_owned();
                        end = output.galley.end().index.0;
                    },
                )
                .drop_without_applying_deltas();
            assert_eq!(end, source_chars);
            (rows, laid_out, source_chars)
        }

        let (wrapped_rows, wrapped_text, _) = layout(true);
        let (unwrapped_rows, unwrapped_text, _) = layout(false);
        assert!(wrapped_rows > logical_line_count(&wrapped_text));
        assert_eq!(unwrapped_rows, logical_line_count(&unwrapped_text));
        assert_eq!(wrapped_text, unwrapped_text);
    }

    #[test]
    fn line_number_gutter_is_off_or_scales_with_digits() {
        assert_eq!(line_number_gutter_width(1, false), 4);
        assert!(line_number_gutter_width(100, true) > line_number_gutter_width(9, true));
        assert!(line_number_gutter_width(usize::MAX, true) <= 120);
    }

    #[test]
    fn production_theme_preference_releases_system_override_but_keeps_explicit_modes() {
        assert_eq!(
            active_theme_preference(InterfaceTheme::System, false, false),
            egui::ThemePreference::System
        );
        assert_eq!(
            active_theme_preference(InterfaceTheme::System, false, true),
            egui::ThemePreference::System
        );
        assert_eq!(
            active_theme_preference(InterfaceTheme::Light, false, false),
            egui::ThemePreference::Light
        );
        assert_eq!(
            active_theme_preference(InterfaceTheme::Dark, false, true),
            egui::ThemePreference::Dark
        );
        assert_eq!(
            active_theme_preference(InterfaceTheme::System, true, false),
            egui::ThemePreference::Light
        );
        assert_eq!(
            active_theme_preference(InterfaceTheme::System, true, true),
            egui::ThemePreference::Dark
        );

        let context = egui::Context::default();
        context.set_theme(active_theme_preference(
            InterfaceTheme::System,
            false,
            false,
        ));
        let output = context.run_ui(
            egui::RawInput {
                system_theme: Some(egui::Theme::Light),
                ..Default::default()
            },
            |_| {},
        );
        assert!(output.viewport_output.values().any(|viewport| {
            viewport.commands.iter().any(|command| {
                matches!(
                    command,
                    egui::ViewportCommand::SetTheme(egui::SystemTheme::SystemDefault)
                )
            })
        }));
        output.drop_without_applying_deltas();
        assert_eq!(context.theme(), egui::Theme::Light);

        for system_theme in [egui::Theme::Dark, egui::Theme::Light] {
            context
                .run_ui(
                    egui::RawInput {
                        system_theme: Some(system_theme),
                        ..Default::default()
                    },
                    |_| {},
                )
                .drop_without_applying_deltas();
            assert_eq!(context.theme(), system_theme);
        }

        context.set_theme(active_theme_preference(InterfaceTheme::Light, false, false));
        context
            .run_ui(
                egui::RawInput {
                    system_theme: Some(egui::Theme::Dark),
                    ..Default::default()
                },
                |_| {},
            )
            .drop_without_applying_deltas();
        assert_eq!(context.theme(), egui::Theme::Light);

        context.set_theme(active_theme_preference(InterfaceTheme::Dark, false, true));
        context
            .run_ui(
                egui::RawInput {
                    system_theme: Some(egui::Theme::Light),
                    ..Default::default()
                },
                |_| {},
            )
            .drop_without_applying_deltas();
        assert_eq!(context.theme(), egui::Theme::Dark);
    }

    #[test]
    fn system_appearance_changes_select_the_configured_theme_pair_in_both_directions() {
        let mut settings = AppSettings {
            interface_theme: InterfaceTheme::System,
            light_theme: ColorThemeChoice::builtin("paper-light"),
            dark_theme: ColorThemeChoice::builtin("catppuccin-mocha"),
            ..AppSettings::default()
        };
        let light = active_theme_request(&settings, Some(egui::Theme::Light), None);
        let dark = active_theme_request(&settings, Some(egui::Theme::Dark), None);
        let light_again = active_theme_request(&settings, Some(egui::Theme::Light), None);
        assert_eq!(
            light.source,
            ThemeSourceRequest::Builtin("paper-light".to_owned())
        );
        assert_eq!(
            dark.source,
            ThemeSourceRequest::Builtin("catppuccin-mocha".to_owned())
        );
        assert_ne!(light, dark);
        assert_eq!(light, light_again);

        settings.interface_theme = InterfaceTheme::Light;
        let fixed_light = active_theme_request(&settings, Some(egui::Theme::Light), None);
        assert_eq!(
            fixed_light,
            active_theme_request(&settings, Some(egui::Theme::Dark), None)
        );
        assert_eq!(fixed_light.source, light.source);

        settings.interface_theme = InterfaceTheme::Dark;
        let fixed_dark = active_theme_request(&settings, Some(egui::Theme::Dark), None);
        assert_eq!(
            fixed_dark,
            active_theme_request(&settings, Some(egui::Theme::Light), None)
        );
        assert_eq!(fixed_dark.source, dark.source);
    }

    #[test]
    fn theme_resolution_uses_the_slot_for_effective_appearance() {
        let mut settings = AppSettings {
            light_theme: ColorThemeChoice::builtin("paper-light"),
            dark_theme: ColorThemeChoice::sublime("/themes/custom.sublime-color-scheme"),
            theme_invert: true,
            theme_hue_shift_degrees: -20,
            ..AppSettings::default()
        };
        let override_profile = CaptureThemeProfile {
            name: "catppuccin-mocha".to_owned(),
            invert: false,
            hue_shift_degrees: 45,
        };

        let launch =
            active_theme_request(&settings, Some(egui::Theme::Light), Some(&override_profile));
        assert_eq!(
            launch.source,
            ThemeSourceRequest::Builtin("catppuccin-mocha".to_owned())
        );
        assert!(!launch.invert);
        assert_eq!(launch.hue_shift_degrees, 45);

        let imported = active_theme_request(&settings, Some(egui::Theme::Dark), None);
        assert_eq!(
            imported.source,
            ThemeSourceRequest::Sublime(PathBuf::from("/themes/custom.sublime-color-scheme"))
        );
        assert!(imported.invert);
        assert_eq!(imported.hue_shift_degrees, -20);

        let builtin = active_theme_request(&settings, Some(egui::Theme::Light), None);
        assert_eq!(
            builtin.source,
            ThemeSourceRequest::Builtin("paper-light".to_owned())
        );

        settings.interface_theme = InterfaceTheme::Light;
        let explicit_light = active_theme_request(&settings, Some(egui::Theme::Dark), None);
        assert_eq!(explicit_light.source, builtin.source);

        settings.interface_theme = InterfaceTheme::Dark;
        let explicit_dark = active_theme_request(&settings, Some(egui::Theme::Light), None);
        assert_eq!(explicit_dark.source, imported.source);
    }

    #[test]
    fn system_theme_selects_a_light_or_dark_tiptop_palette() {
        let settings = AppSettings::default();
        let light = active_theme_request(&settings, Some(egui::Theme::Light), None);
        let dark = active_theme_request(&settings, Some(egui::Theme::Dark), None);
        assert_eq!(
            light.source,
            ThemeSourceRequest::Builtin("tiptop-light".to_owned())
        );
        assert_eq!(
            dark.source,
            ThemeSourceRequest::Builtin("tiptop-dark".to_owned())
        );
    }

    #[test]
    fn document_theme_maps_to_tinymist_with_effective_preview_appearance() {
        assert_eq!(
            tinymist_invert_colors(DocumentTheme::FollowInterface, false),
            InvertColors::Never
        );
        assert_eq!(
            tinymist_invert_colors(DocumentTheme::Light, true),
            InvertColors::Never
        );
        assert_eq!(
            tinymist_invert_colors(DocumentTheme::Dark, false),
            InvertColors::Always
        );
        assert_eq!(
            tinymist_invert_colors(DocumentTheme::FollowInterface, true),
            InvertColors::Always
        );
    }

    #[test]
    fn resolved_system_appearance_change_restarts_tinymist_preview() {
        assert!(!tinymist_restart_required(
            false, false, false, false, false
        ));
        assert!(tinymist_restart_required(true, false, false, false, false));
        assert!(tinymist_restart_required(false, true, false, false, false));
    }

    #[test]
    fn activation_sensitive_webview_creation_waits_only_for_explicit_blur() {
        assert!(!may_create_embedded_webview(true, Some(false)));
        assert!(may_create_embedded_webview(true, Some(true)));
        assert!(may_create_embedded_webview(true, None));
        assert!(may_create_embedded_webview(false, Some(false)));
    }

    #[test]
    fn every_secondary_window_can_embed_after_its_native_parent_is_focused() {
        assert!(!may_create_window_webview(
            EditorWindowHost::Secondary,
            true,
            None,
        ));
        assert!(!may_create_window_webview(
            EditorWindowHost::Secondary,
            false,
            Some(false),
        ));
        assert!(may_create_window_webview(
            EditorWindowHost::Secondary,
            true,
            Some(true),
        ));
        assert!(may_create_window_webview(
            EditorWindowHost::Root,
            false,
            Some(false),
        ));
    }

    #[test]
    fn sublime_slot_mode_is_validated_before_color_transforms() {
        let temp = tempfile::tempdir().expect("create temporary theme directory");
        let path = temp.path().join("Dark.sublime-color-scheme");
        fs::write(
            &path,
            r##"{
                "name": "Test Dark",
                "globals": {
                    "background": "#151820",
                    "foreground": "#edf1f7"
                }
            }"##,
        )
        .expect("write dark Sublime theme");

        let mismatched = ActiveThemeRequest {
            source: ThemeSourceRequest::Sublime(path.clone()),
            invert: true,
            hue_shift_degrees: 30,
            fallback_dark: false,
        };
        let error = load_active_theme(&mismatched).expect_err("dark file is not a light choice");
        assert!(error.contains("dark Sublime theme"));
        assert!(error.contains("light slot"));

        let matching = ActiveThemeRequest {
            fallback_dark: true,
            ..mismatched
        };
        let transformed = load_active_theme(&matching).expect("dark slot accepts the dark file");
        assert!(
            !transformed.dark_mode,
            "inversion runs only after validation"
        );
    }

    #[test]
    fn active_theme_transform_is_ordered_and_non_cumulative() {
        let request = ActiveThemeRequest {
            source: ThemeSourceRequest::Builtin("catppuccin-latte".to_owned()),
            invert: true,
            hue_shift_degrees: 30,
            fallback_dark: false,
        };
        let original = builtin_themes::find("catppuccin-latte").unwrap();
        let expected = request.transform().apply_rgba(original.palette.background);
        let first = load_active_theme(&request).unwrap();
        let second = load_active_theme(&request).unwrap();

        assert_eq!(first.palette.background, expected);
        assert_eq!(first.palette, second.palette);
        assert_eq!(first.syntect_theme, second.syntect_theme);
        assert!(first.dark_mode);
    }

    #[test]
    fn unknown_themes_report_an_error_and_use_the_paired_fallback() {
        let request = ActiveThemeRequest {
            source: ThemeSourceRequest::Builtin("does-not-exist".to_owned()),
            invert: false,
            hue_shift_degrees: 0,
            fallback_dark: true,
        };
        let (fallback, error) = load_active_theme_or_fallback(&request);
        assert!(error.unwrap().contains("does-not-exist"));
        assert_eq!(fallback.name.as_deref(), Some("Tiptop Dark"));
        assert!(fallback.dark_mode);
    }

    #[test]
    fn light_and_dark_styles_have_identical_layout_geometry() {
        let context = egui::Context::default();
        theme::configure_styles(&context);
        let dark = context.style_of(egui::Theme::Dark);
        let light = context.style_of(egui::Theme::Light);

        assert_eq!(dark.spacing, light.spacing);
        assert_eq!(dark.text_styles, light.text_styles);
        assert_eq!(dark.spacing.item_spacing, egui::vec2(6.0, 4.0));
        assert_eq!(dark.spacing.button_padding, egui::vec2(7.0, 3.0));
        assert_eq!(
            dark.text_styles.get(&egui::TextStyle::Monospace),
            Some(&theme::editor_font())
        );
    }

    #[test]
    fn automatic_preview_fallback_preserves_and_reports_user_intent() {
        let preference = PreviewPreference::Interactive;
        let tinymist = ServiceState::Failed("tinymist executable was not found".to_owned());
        let webview = ServiceState::Starting("waiting".to_owned());

        assert_eq!(
            preview_fallback_reason_for(preference, false, false, &tinymist, &webview),
            Some("Failed: tinymist executable was not found".to_owned())
        );
        assert_eq!(
            preview_backend_label_for(preference, false),
            "Rasterised PDF · fallback"
        );
        assert_eq!(preference, PreviewPreference::Interactive);
    }

    #[test]
    fn explicitly_selected_native_preview_is_not_a_fallback() {
        let tinymist = ServiceState::Failed("unavailable".to_owned());
        let webview = ServiceState::Failed("unavailable".to_owned());

        assert_eq!(
            preview_fallback_reason_for(
                PreviewPreference::Native,
                false,
                false,
                &tinymist,
                &webview,
            ),
            None
        );
        assert_eq!(
            preview_backend_label_for(PreviewPreference::Native, false),
            "Rasterised PDF"
        );
    }

    #[test]
    fn embedded_viewer_failure_is_reported_after_preview_server_startup() {
        let tinymist = ServiceState::Ready("server ready".to_owned());
        let webview = ServiceState::Failed("child webview could not load".to_owned());

        assert_eq!(
            preview_fallback_reason_for(
                PreviewPreference::Interactive,
                false,
                true,
                &tinymist,
                &webview,
            ),
            Some("Failed: child webview could not load".to_owned())
        );
    }

    #[test]
    fn bottom_status_collects_every_non_preview_fallback() {
        let settings = AppSettings::default();
        let typst = ToolResolution {
            kind: ToolKind::Typst,
            program: PathBuf::from("typst"),
            origin: ToolOrigin::Path,
            fallback_reason: Some("bundled Typst is missing; using PATH".to_owned()),
        };
        let tinymist = ToolResolution {
            kind: ToolKind::Tinymist,
            program: PathBuf::from("tinymist"),
            origin: ToolOrigin::Missing,
            fallback_reason: Some("Tinymist is unavailable".to_owned()),
        };

        let details = non_preview_fallback_details_for(&settings, None, &typst, &tinymist);
        assert_eq!(details.len(), 3);
        assert!(details[0].starts_with("Appearance:"));
        assert_eq!(details[1], "Typst: bundled Typst is missing; using PATH");
        assert_eq!(details[2], "Tinymist: Tinymist is unavailable");
    }

    #[test]
    fn tinymist_diagnostics_become_one_based_inline_diagnostics() {
        let converted = tinymist_diagnostic(
            TinymistDiagnostic {
                range: LspRange {
                    start: LspPosition {
                        line: 4,
                        character: 7,
                    },
                    end: LspPosition {
                        line: 4,
                        character: 11,
                    },
                },
                severity: Some(TinymistDiagnosticSeverity::Warning),
                code: Some(serde_json::json!("deprecated")),
                source: Some("tinymist".to_owned()),
                message: "old syntax".to_owned(),
                raw: serde_json::json!({}),
            },
            DiagnosticSource::Main,
        );

        assert_eq!(converted.severity, DiagnosticSeverity::Warning);
        assert_eq!(converted.location.unwrap().line, 5);
        assert_eq!(converted.location.unwrap().column, 8);
        assert_eq!(converted.full_message(), "old syntax\ncode: \"deprecated\"");
    }
}
