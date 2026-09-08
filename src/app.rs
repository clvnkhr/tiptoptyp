use std::{
    collections::{BTreeMap, VecDeque, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use eframe::egui::{
    self, Align, Color32, ColorImage, KeyboardShortcut, Layout, Modifiers, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, TextureOptions, Vec2,
    text::{CCursor, CCursorRange},
};
use egui_ltreeview::{Action as TreeAction, NodeBuilder, TreeView, TreeViewBuilder, TreeViewState};
use rfd::AsyncFileDialog;

use crate::{
    asset::{AssetLoader, LoadedAsset},
    builtin_themes,
    child_view::{ChildViewHost, ChildViewSpec, scoped_child_viewport_id, viewport_scoped_id},
    compiler::{ArtifactKey, CompileEvent, CompileRequest, Compiler, PreviewPage},
    diagnostics::{
        Diagnostic, DiagnosticLocation, DiagnosticSeverity, DiagnosticSource,
        normalize_diagnostics, parse_typst_short_output,
    },
    document::{DocumentKind, DocumentSession, EditorSnapshot},
    editor_data::{EditorDerivedData, EditorRevision, FontArgumentTarget, LineDiagnostic},
    font_catalog::{FontCatalog, FontFamily, ignored_workspace_directory, is_font_path},
    generic_highlight::GenericSyntaxHighlighter,
    highlight::SyntaxHighlighter,
    lsp_text::{
        apply_text_edits, lsp_position_at_char, range_to_char_range, scalar_position_at_char,
    },
    native_menu::{
        AppCommand, CommandMenu, CommandRequirement, NativeMenuCommandQueue, command_spec,
        command_specs, consume_shortcut,
    },
    presentation::{
        ActiveThemeRequest, AppliedPresentation, ResolvedPresentationRequest,
        active_theme_preference, active_theme_request, load_active_theme_or_fallback,
        theme_request_for_appearance,
    },
    preview::{
        PAGE_MARGIN, PDF_POINTS_PER_PREVIEW_PIXEL, PreviewController, PreviewStatus,
        PreviewTexture, RasterContentFreshness, ServiceState, dark_preview_rgba,
        page_stack_geometry, stack_height, visible_page, zoom_anchored_offset,
    },
    project_index::{ProjectIndex, analyze_project},
    screenshot::{CaptureController, CaptureThemeProfile, UiCaptureStep, UiSnapshotScene},
    search::{SearchRevision, SearchSession},
    settings::{
        AppSettings, ColorThemeChoice, DEFAULT_HOVER_DELAY_MS, DEFAULT_HOVER_FADE_MS,
        DEFAULT_UI_FONT_WEIGHT, DEFAULT_UI_SCALE_PERCENT, DocumentTheme, InterfaceTheme,
        PreviewPreference, SourcePreviewTrigger, ToolMode, ToolPreference,
    },
    sublime_theme::{self, ImportedTheme, Rgba},
    syntax_theme::{ResolvedTypstStyles, TypstStyleOverride, TypstStyleOverrides, TypstSyntaxRole},
    theme::{self, METRICS},
    tinymist::{
        DiagnosticSeverity as TinymistDiagnosticSeverity, Generation, InvertColors, LspRange,
        LspTextEdit, TextDocument, TinymistConfig, TinymistDiagnostic, TinymistEvent,
        TinymistSidecar, UnsavedTextDocument,
    },
    toolchain::{ToolKind, ToolOrigin, ToolResolution, resolve_tool},
    worker::{LatestJob, LatestJobPoll},
    workflow::{
        AppModal, AppModalChoice, DeferredDocumentAction, DialogPoll, DocumentDialogRequest,
        DocumentDialogTarget, DocumentWorkflow, ExportDialogRequest, NoticeKind, PdfWriteIntent,
        PendingDialog, PendingDocumentAction, PendingExport, poll_dialog,
    },
    workspace::{WorkspaceNode, WorkspaceSnapshot, WorkspaceTree},
};

#[cfg(test)]
use crate::presentation::{ThemeSourceRequest, load_active_theme};
#[cfg(test)]
use crate::preview::{
    preview_backend_label_for, preview_fallback_reason_for, raster_content_freshness,
    raster_result_matches_artifact,
};

const COMPILE_DEBOUNCE: Duration = Duration::from_millis(60);
const WORKSPACE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const AUTOSAVE_RETRY_DELAY: Duration = Duration::from_secs(2);
const PROJECT_INDEX_DEBOUNCE: Duration = Duration::from_millis(180);
const EXTERNAL_FILE_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const MIN_PREVIEW_ZOOM: f32 = 0.2;
const MAX_PREVIEW_ZOOM: f32 = 6.0;
const TOOLTIP_HANDOFF_GRACE: Duration = Duration::from_millis(300);
const POPUP_BLUR_GRACE: Duration = Duration::from_millis(120);
const STATUS_LOG_LIMIT: usize = 100;
const STATUS_LOG_TIMESTAMP_WIDTH: f32 = 74.0;
const STATUS_LOG_ROW_HEIGHT: f32 = 24.0;

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

fn typst_preview_available_for(document_kind: DocumentKind, designated: bool) -> bool {
    document_kind.is_typst() || designated
}

fn preview_visible_for(document_kind: DocumentKind, view_mode: ViewMode, designated: bool) -> bool {
    document_kind.preview_only()
        || (typst_preview_available_for(document_kind, designated) && view_mode.shows_preview())
}

#[cfg(test)]
const fn raster_preview_required_for(
    interactive_requested: bool,
    interactive_unavailable: bool,
    screenshot_pending: bool,
) -> bool {
    screenshot_pending || !interactive_requested || interactive_unavailable
}

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
const fn raster_fallback_compile_needed(
    typst_preview_available: bool,
    compilation_allowed: bool,
    raster_was_required: bool,
    raster_is_required: bool,
) -> bool {
    typst_preview_available && compilation_allowed && !raster_was_required && raster_is_required
}

const fn compilation_run_allowed(
    paused: bool,
    export_pending: bool,
    screenshot_pending: bool,
) -> bool {
    !paused || export_pending || screenshot_pending
}

fn default_compile_pdf_path(
    designated_preview: Option<&Path>,
    document_kind: DocumentKind,
    document_path: Option<&Path>,
) -> Option<PathBuf> {
    let source = designated_preview.or_else(|| {
        if document_kind.is_typst() {
            document_path
        } else {
            None
        }
    })?;
    let mut output = source.to_path_buf();
    output.set_extension("pdf");
    Some(output)
}

const fn compilation_toggle_copy(paused: bool) -> (&'static str, &'static str) {
    if paused {
        ("Resume", "Resume automatic preview updates")
    } else {
        (
            "Pause",
            "Pause automatic preview updates; Compile PDF stays available",
        )
    }
}

const fn compilation_notice(paused: bool) -> (&'static str, NoticeKind) {
    if paused {
        ("Automatic preview updates paused", NoticeKind::Info)
    } else {
        ("Automatic preview updates resumed", NoticeKind::Success)
    }
}

const fn snapshot_scene_hides_preview_pages(scene: Option<UiSnapshotScene>) -> bool {
    matches!(scene, Some(UiSnapshotScene::PreviewCompiling))
}

const fn settled_snapshot_preview_status(
    scene: UiSnapshotScene,
    has_pages: bool,
) -> Option<PreviewStatus> {
    if !has_pages {
        // Do not replace Waiting/Compiling while the deterministic fixture is
        // still producing the raster that gates its main-viewport capture.
        return None;
    }
    match scene {
        UiSnapshotScene::Main | UiSnapshotScene::FindReplace => {
            Some(PreviewStatus::Ready(Duration::ZERO))
        }
        UiSnapshotScene::ProblemsPanel => Some(PreviewStatus::Error),
        _ => None,
    }
}

const fn capture_preview_build_needed(
    main_capture_pending: bool,
    has_pages: bool,
    compile_scheduled: bool,
    status: PreviewStatus,
    has_artifact: bool,
) -> bool {
    main_capture_pending
        && !has_pages
        && !compile_scheduled
        && !has_artifact
        && !matches!(status, PreviewStatus::Compiling | PreviewStatus::Error)
}

const fn retain_preview_surface_for_restart(
    handoff_requested: bool,
    preview_will_restart: bool,
    has_preview_url: bool,
    has_webview: bool,
) -> bool {
    handoff_requested && preview_will_restart && has_preview_url && has_webview
}

fn webview_navigation_required(
    current_url: Option<&str>,
    next_url: &str,
    reload_pending: bool,
) -> bool {
    reload_pending || current_url != Some(next_url)
}

fn source_preview_jump_gesture(
    trigger: SourcePreviewTrigger,
    clicked: bool,
    double_clicked: bool,
    command: bool,
    web_link_clicked: bool,
) -> bool {
    if web_link_clicked {
        return false;
    }
    match trigger {
        SourcePreviewTrigger::DoubleClick => double_clicked,
        SourcePreviewTrigger::ModifierClick => clicked && command,
        SourcePreviewTrigger::Disabled => false,
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewNavigationContext {
    base_url: String,
    project_root: PathBuf,
    source_dir: Option<PathBuf>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreviewNavigationAction {
    Embed,
    Dispatch(String),
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
enum SaveIntent {
    Explicit,
    ExplicitConfirmed,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    message: String,
    kind: NoticeKind,
}

#[derive(Debug, Clone)]
struct StatusLogEntry {
    timestamp: String,
    detail: String,
    kind: NoticeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalFileObservation {
    Present(u64),
    Missing,
}

#[derive(Debug, Clone, Copy)]
struct EditorAttention {
    char_index: usize,
    started: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FindStep {
    Next,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewZoomAction {
    In,
    Out,
    Reset,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownInlineSpan {
    text: String,
    code: bool,
    bold: bool,
    italics: bool,
    link: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct TooltipGeometry {
    identity: u64,
    origin: Rect,
    card: Rect,
    pointer_inside_viewport: bool,
    handoff_until: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TooltipInteractionState {
    identity: u64,
    focused: bool,
    had_focus: bool,
    focus_requested: bool,
    dismissed: bool,
}

impl TooltipInteractionState {
    const fn new(identity: u64) -> Self {
        Self {
            identity,
            focused: false,
            had_focus: false,
            focus_requested: false,
            dismissed: false,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPickerTarget {
    Typst,
    Tinymist,
    SublimeTheme { dark_mode: bool },
    UiFont,
    CodeFont,
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
    OpenInNewWindow(PathBuf),
    TogglePreview(PathBuf),
    Rename(PathBuf),
    Copy {
        path: PathBuf,
        kind: WorkspaceCopyKind,
    },
    Reveal(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceCopyKind {
    FileName,
    FilePath,
    RelativePath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EditorMenuAction {
    Command(AppCommand),
    OpenLink(String),
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
    View {
        anchor: Pos2,
    },
    Workspace {
        anchor: Pos2,
        path: PathBuf,
        is_file: bool,
    },
    Editor {
        anchor: Pos2,
        link: Option<String>,
    },
    FontSelector {
        anchor: Pos2,
        target: FontArgumentTarget,
    },
    StatusLog {
        anchor: Pos2,
    },
}

#[derive(Debug, Clone)]
enum AppPopupAction {
    Command(AppCommand),
    Editor(EditorMenuAction),
    Workspace(WorkspaceMenuAction),
    SetDocumentFont {
        target: FontArgumentTarget,
        family: String,
    },
}

pub struct EditorApp {
    document: DocumentSession,
    highlighter: SyntaxHighlighter,
    generic_highlighter: GenericSyntaxHighlighter,

    compiler: Compiler,
    asset_loader: AssetLoader,
    asset_token: u64,
    pending_asset_page: Option<usize>,
    compile_deadline: Option<Instant>,
    compilation_paused: bool,
    preview: PreviewController,
    editor_data: EditorDerivedData,

    view_mode: ViewMode,
    filesystem_visible: bool,
    problems_visible: bool,
    settings_visible: bool,
    typst_overrides_visible: bool,
    typst_overrides_dark: bool,
    settings: AppSettings,
    pending_settings: Option<AppSettings>,
    imported_theme: Option<ImportedTheme>,
    presentation: AppliedPresentation,
    theme_override: Option<CaptureThemeProfile>,
    font_configuration: theme::FontConfiguration,
    font_catalog: FontCatalog,
    font_catalog_revision: u64,
    font_catalog_scan: LatestJob<(PathBuf, FontCatalog)>,
    font_catalog_root: PathBuf,
    staged_ui_font_weight: Option<u16>,
    staged_code_font_weight: Option<u16>,
    tool_refresh_requested: bool,
    typst_tool: ToolResolution,
    tinymist_tool: ToolResolution,
    workspace_root: PathBuf,
    workspace_chooser_visible: bool,
    workspace: Option<WorkspaceTree>,
    workspace_error: Option<String>,
    workspace_scan: LatestJob<WorkspaceSnapshot>,
    next_workspace_refresh: Instant,
    project_index: ProjectIndex,
    project_index_deadline: Option<Instant>,
    project_index_job: LatestJob<ProjectIndex>,
    captures: CaptureController,
    snapshot_scene: Option<UiSnapshotScene>,
    window_host: EditorWindowHost,
    pending_window_requests: VecDeque<EditorWindowRequest>,
    queued_native_menu_commands: NativeMenuCommandQueue,
    queued_open_requests: VecDeque<PathBuf>,

    find_visible: bool,
    replace_visible: bool,
    find_query: String,
    replacement: String,
    search: SearchSession,
    find_case_sensitive: bool,
    find_regex: bool,
    focus_find: bool,
    pending_editor_selection: Option<Range<usize>>,
    editor_attention: Option<EditorAttention>,
    editor_hover: Option<EditorHoverState>,
    next_editor_hover_token: u64,
    manual_format_revision: Option<u64>,
    autosave_deadline: Option<Instant>,
    diagnostic_tooltip: Option<DiagnosticTooltipOverlay>,
    app_popup: Option<AppPopup>,
    app_popup_generation: u64,
    app_popup_had_focus: bool,
    app_popup_blur_started: Option<Instant>,
    pending_app_popup_action: Option<AppPopupAction>,
    document_workflow: DocumentWorkflow,
    rename_dialog: Option<RenameDialog>,
    rename_overlay_had_focus: bool,
    rename_overlay_suspended: bool,

    pending_tool_picker: Option<PendingDialog<ToolPickerTarget>>,
    notice: Option<Notice>,
    status_log: VecDeque<StatusLogEntry>,
    recorded_status: Option<PreviewStatus>,
    recorded_notice: Option<Notice>,
    next_external_file_check: Instant,
    external_file_change_notice: Option<ExternalFileObservation>,
    last_title: String,

    tinymist: TinymistSidecar,
    tinymist_generation: Option<Generation>,
    tinymist_uri: Option<String>,
    tinymist_preview_uri: Option<String>,
    tinymist_current_open: bool,
    tinymist_unsaved_document: Option<UnsavedTextDocument>,

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    native_window_parent: Option<crate::native_window::ActiveWindowHandle>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview: Option<wry::WebView>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview_url: Option<String>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview_reload_pending: bool,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview_navigation: Option<Arc<Mutex<PreviewNavigationContext>>>,
    web_link_sender: mpsc::Sender<String>,
    web_link_receiver: mpsc::Receiver<String>,
}

impl EditorApp {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        captures: CaptureController,
        theme_override: Option<CaptureThemeProfile>,
        snapshot_scene: Option<UiSnapshotScene>,
    ) -> Self {
        Self::new_session(
            &context.egui_ctx,
            initial_path,
            captures,
            theme_override,
            snapshot_scene,
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
        mut settings: AppSettings,
        window_host: EditorWindowHost,
    ) -> Self {
        if snapshot_scene.is_some() {
            settings.ui_font_weight = DEFAULT_UI_FONT_WEIGHT;
            settings.code_font_weight = DEFAULT_UI_FONT_WEIGHT;
        }
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
        let presentation = AppliedPresentation::new(ResolvedPresentationRequest::resolve(
            &settings,
            applied_theme_request,
            0,
        ));
        theme::set_imported_palette(Some((active_theme.dark_mode, active_theme.palette)));
        let typst_tool = resolve_tool(ToolKind::Typst, &settings.typst);
        let tinymist_tool = resolve_tool(ToolKind::Tinymist, &settings.tinymist);
        context.options_mut(|options| {
            options.zoom_with_keyboard = false;
            options.sync_window_theme = true;
            options.fallback_theme = egui::Theme::Dark;
        });
        let font_configuration = theme::configure_editor_fonts(
            context,
            theme::FontRequest {
                fallback_path: settings.ui_font_path.as_deref().map(Path::new),
                fallback_face_index: settings.ui_font_face_index,
                ..Default::default()
            },
            theme::FontRequest {
                fallback_path: settings.code_font_path.as_deref().map(Path::new),
                fallback_face_index: settings.code_font_face_index,
                ..Default::default()
            },
            settings.ui_font_monospace,
            settings.ui_font_weight,
            settings.code_font_weight,
        );
        theme::configure_styles(context);
        theme::configure_ui_font(context, font_configuration.weighted_ui_loaded);
        apply_ui_scale(context, settings.ui_scale_percent);
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
            document: DocumentSession::new(DEFAULT_SOURCE, DocumentKind::Typst),
            highlighter,
            generic_highlighter,
            compiler: Compiler::new(context.clone()),
            asset_loader: AssetLoader::new(context.clone()),
            asset_token: 0,
            pending_asset_page: None,
            compile_deadline: Some(Instant::now()),
            compilation_paused: false,
            preview: PreviewController::new(preview_dark, settings.preview_preference),
            editor_data: EditorDerivedData::default(),
            view_mode: ViewMode::Split,
            filesystem_visible: true,
            problems_visible: false,
            settings_visible: false,
            typst_overrides_visible: false,
            typst_overrides_dark: active_theme.dark_mode,
            settings: settings.clone(),
            pending_settings: None,
            imported_theme: Some(active_theme),
            presentation,
            theme_override,
            font_configuration,
            font_catalog: FontCatalog::default(),
            font_catalog_revision: 0,
            font_catalog_scan: LatestJob::default(),
            font_catalog_root: workspace_root.clone(),
            staged_ui_font_weight: None,
            staged_code_font_weight: None,
            tool_refresh_requested: false,
            typst_tool,
            tinymist_tool,
            workspace_root,
            workspace_chooser_visible: false,
            workspace: None,
            workspace_error: None,
            workspace_scan: LatestJob::default(),
            next_workspace_refresh: Instant::now(),
            project_index: ProjectIndex::default(),
            project_index_deadline: Some(Instant::now()),
            project_index_job: LatestJob::default(),
            captures,
            snapshot_scene,
            window_host,
            pending_window_requests: VecDeque::new(),
            queued_native_menu_commands: NativeMenuCommandQueue::default(),
            queued_open_requests: VecDeque::new(),
            find_visible: false,
            replace_visible: false,
            find_query: String::new(),
            replacement: String::new(),
            search: SearchSession::default(),
            find_case_sensitive: true,
            find_regex: false,
            focus_find: false,
            pending_editor_selection: None,
            editor_attention: None,
            editor_hover: None,
            next_editor_hover_token: 1,
            manual_format_revision: None,
            autosave_deadline: None,
            diagnostic_tooltip: None,
            app_popup: None,
            app_popup_generation: 0,
            app_popup_had_focus: false,
            app_popup_blur_started: None,
            pending_app_popup_action: None,
            document_workflow: DocumentWorkflow::default(),
            rename_dialog: None,
            rename_overlay_had_focus: false,
            rename_overlay_suspended: false,
            pending_tool_picker: None,
            notice: None,
            status_log: VecDeque::new(),
            recorded_status: None,
            recorded_notice: None,
            next_external_file_check: Instant::now(),
            external_file_change_notice: None,
            last_title: String::new(),
            tinymist: TinymistSidecar::new(context.clone()),
            tinymist_generation: None,
            tinymist_uri: None,
            tinymist_preview_uri: None,
            tinymist_current_open: false,
            tinymist_unsaved_document: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            native_window_parent: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview_url: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview_reload_pending: false,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview_navigation: None,
            web_link_sender,
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
        if app.snapshot_scene.is_some() {
            // Snapshot scenes must not depend on how quickly the host font
            // directories can be traversed. A compact catalog also keeps the
            // font-selector scene useful and deterministic.
            app.font_catalog = FontCatalog::snapshot_fixture();
            app.font_catalog_revision = app.font_catalog_revision.wrapping_add(1);
            app.font_catalog_root = app.project_root();
        } else {
            app.request_font_catalog_scan(context);
        }
        app
    }

    pub(crate) fn new_secondary(
        context: &egui::Context,
        request: EditorWindowRequest,
        settings: AppSettings,
        captures: CaptureController,
    ) -> Self {
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
    pub(crate) fn enqueue_native_menu_command(&mut self, command: AppCommand) {
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
        self.document.path.is_none() && !self.is_dirty() && !self.document_flow_busy()
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
        self.document_workflow.allow_close
    }

    pub(crate) fn show_window_notice(&mut self, message: String) {
        self.notice = Some(Notice {
            message,
            kind: NoticeKind::Info,
        });
    }

    fn is_dirty(&self) -> bool {
        self.document.is_dirty()
    }

    fn document_name(&self) -> String {
        self.document.name()
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
        self.document
            .path
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

    fn request_font_catalog_scan(&mut self, context: &egui::Context) {
        let root = self.project_root();
        self.font_catalog_root = root.clone();
        if let Err(error) =
            self.font_catalog_scan
                .start_and_repaint("tiptoptyp-font-catalog", context, move || {
                    Ok((root.clone(), FontCatalog::discover(&root)))
                })
        {
            self.notice = Some(Notice {
                message: format!("Could not start font discovery: {error}"),
                kind: NoticeKind::Error,
            });
        }
    }

    fn poll_font_catalog(&mut self, context: &egui::Context) {
        if self.snapshot_scene.is_some() {
            self.font_catalog_scan.cancel();
            return;
        }
        if self.font_catalog_root != self.project_root() {
            self.request_font_catalog_scan(context);
        }
        let (root, catalog) = match self.font_catalog_scan.poll() {
            LatestJobPoll::Idle => return,
            LatestJobPoll::Pending => {
                context.request_repaint_after(Duration::from_millis(100));
                return;
            }
            LatestJobPoll::Ready(result) => result,
            LatestJobPoll::Failed(error) => {
                self.notice = Some(Notice {
                    message: format!("Font discovery stopped unexpectedly: {error}"),
                    kind: NoticeKind::Error,
                });
                return;
            }
        };
        if root != self.project_root() {
            return;
        }
        let font_files_changed = !self
            .font_catalog
            .workspace_files_match(catalog.workspace_files());
        self.font_catalog = catalog;
        self.font_catalog_revision = self.font_catalog_revision.wrapping_add(1);
        if font_files_changed {
            self.restart_tinymist();
            self.schedule_compile_now();
        }
    }

    fn selected_font_family(
        &self,
        path: Option<&str>,
        family: Option<&str>,
    ) -> Option<&FontFamily> {
        self.font_catalog.selected_family(Path::new(path?), family)
    }

    fn designated_preview_path(&self) -> Option<PathBuf> {
        // A QA scene previews its explicit fixture, regardless of the user's
        // saved project entry. Do not mutate the persisted setting.
        if self.snapshot_scene.is_some() {
            return None;
        }
        let root = canonical_or_absolute(&self.workspace_root);
        let key = root.to_str()?;
        let value = self.settings.preview_files.get(key)?;
        let path = canonical_or_absolute(Path::new(value));
        (path.starts_with(&root)
            && path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("typ")))
        .then_some(path)
    }

    fn preview_document_path(&self) -> PathBuf {
        self.designated_preview_path()
            .or_else(|| {
                (self.document.kind.is_typst())
                    .then(|| self.document.path.clone())
                    .flatten()
            })
            .unwrap_or_else(|| {
                self.project_root()
                    .join(".tiptoptyp")
                    .join("documents")
                    .join("untitled.typ")
            })
    }

    fn typst_preview_available(&self) -> bool {
        typst_preview_available_for(self.document.kind, self.designated_preview_path().is_some())
    }

    fn current_is_preview_document(&self) -> bool {
        match &self.document.path {
            Some(path) => same_path(path, &self.preview_document_path()),
            None => self.designated_preview_path().is_none() && self.document.kind.is_typst(),
        }
    }

    fn preview_document_source(&self) -> Result<String, String> {
        let path = self.preview_document_path();
        if self.current_is_preview_document() {
            return Ok(self.document.source.clone());
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
        if self.document.kind.is_typst() {
            let path = self.document.path.clone().unwrap_or_else(|| main.clone());
            overrides.insert(canonical_or_absolute(&path), self.document.source.clone());
        }
        if let Err(error) = self.project_index_job.start_and_repaint(
            "tiptoptyp-project-index",
            context,
            move || Ok(analyze_project(&root, &main, &overrides)),
        ) {
            self.notice = Some(Notice {
                message: format!("Could not start project index: {error}"),
                kind: NoticeKind::Error,
            });
        }
    }

    fn tick_project_index(&mut self, context: &egui::Context) {
        match self.project_index_job.poll() {
            LatestJobPoll::Ready(index) if self.project_index_deadline.is_none() => {
                self.project_index = index;
            }
            LatestJobPoll::Failed(error) => {
                self.notice = Some(Notice {
                    message: format!("Project index stopped: {error}"),
                    kind: NoticeKind::Error,
                });
            }
            LatestJobPoll::Idle | LatestJobPoll::Pending | LatestJobPoll::Ready(_) => {}
        }
        if self.project_index_job.is_running() {
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

    fn toggle_file_for_preview(&mut self, path: PathBuf, context: &egui::Context) {
        let root = canonical_or_absolute(&self.workspace_root);
        let path = canonical_or_absolute(&path);
        if !path.starts_with(&root)
            || !path.is_file()
            || path
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("typ"))
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
        let selected = self
            .designated_preview_path()
            .is_some_and(|current| same_path(&current, &path));
        if selected {
            self.settings.preview_files.remove(key);
        } else {
            self.settings
                .preview_files
                .insert(key.to_owned(), value.to_owned());
        }
        if let Some(settings) = &mut self.pending_settings {
            if selected {
                settings.preview_files.remove(key);
            } else {
                settings
                    .preview_files
                    .insert(key.to_owned(), value.to_owned());
            }
        }
        self.rebuild_project_index(context);
        self.restart_tinymist_preserving_preview();
        self.schedule_compile_now();
        self.notice = Some(Notice {
            message: if selected {
                "Preview follows the active Typst document again".to_owned()
            } else {
                format!("Using {} for preview", path.display())
            },
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
        self.manual_format_revision = None;
        self.document.mark_edited();
        self.notice = None;
        self.editor_hover = None;
        if self.document.kind.is_typst() {
            // Tinymist sees unsaved edits in every open source file. The CLI
            // fallback can only see an imported subfile after it is saved, so
            // avoid rebuilding the designated main entry with stale disk data.
            self.compile_deadline = (self.current_is_preview_document()
                && self.preview_processing_enabled()
                && self.may_run_compilation())
            .then(|| Instant::now() + COMPILE_DEBOUNCE);
            if self.compile_deadline.is_some() {
                self.preview.status = PreviewStatus::Waiting;
            }
            if !self.compilation_paused
                && let Err(error) = self.sync_tinymist_change()
            {
                self.preview.tinymist_state = ServiceState::Degraded(error);
            }
            self.schedule_project_index();
        } else {
            self.compile_deadline = None;
        }
        self.schedule_autosave_if_needed();
    }

    fn schedule_autosave_if_needed(&mut self) {
        self.autosave_deadline = (self.settings.auto_save
            && self.document.path.is_some()
            && self.document.source != self.document.saved_source)
            .then(|| {
                Instant::now() + Duration::from_millis(self.settings.auto_save_delay_ms.max(100))
            });
    }

    fn tick_autosave(&mut self, context: &egui::Context) {
        if self.document_workflow.pending_dialog.is_some() || self.document_workflow.modal.is_some()
        {
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
        let Some(path) = self.document.path.clone() else {
            return;
        };

        if self.save_to_with_intent(path, SaveIntent::Auto) {
            self.notice = Some(Notice {
                message: "Saved automatically".to_owned(),
                kind: NoticeKind::Success,
            });
        }
    }

    fn editor_revision(&self) -> EditorRevision {
        EditorRevision::new(self.document.epoch, self.document.revision)
    }

    fn prepare_editor_source_data(&mut self) {
        let revision = self.editor_revision();
        self.editor_data
            .prepare_source(revision, &self.document.source);
    }

    fn prepare_editor_data(&mut self) {
        self.prepare_editor_source_data();
        let document = self.editor_revision();
        let current_path = self.document.path.clone();
        let virtual_path = self.tinymist_document_path();
        let current_is_preview = self.current_is_preview_document();
        self.editor_data.prepare_diagnostics(
            document,
            self.preview.diagnostics_generation,
            current_path.as_deref(),
            &virtual_path,
            current_is_preview,
            &self.preview.diagnostics,
            &self.preview.tinymist_diagnostics,
        );
    }

    fn mark_diagnostics_changed(&mut self) {
        self.preview.mark_diagnostics_changed();
    }

    fn request_compile(&mut self) {
        self.compile_deadline = None;
        if !self.preview_processing_enabled() || !self.may_run_compilation() {
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
            revision: self.document.revision,
            rasterize: self.raster_preview_required(),
            source,
            project_root: self.project_root(),
            source_dir,
            display_name,
            typst_executable: self.typst_tool.program.clone(),
            font_paths: self.font_catalog.workspace_directories().to_vec(),
        };

        match self.compiler.request(request) {
            Ok(()) => self.preview.status = PreviewStatus::Compiling,
            Err(error) => self.set_compile_error(error),
        }
    }

    fn tick_compile(&mut self, context: &egui::Context) {
        if !self.preview_processing_enabled() || !self.may_run_compilation() {
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
            if !self.preview_processing_enabled()
                || !self.may_run_compilation()
                || result.revision != self.document.revision
            {
                continue;
            }

            match result.event {
                CompileEvent::Started => {
                    // Keep the previous artifact and pages until their
                    // independently versioned replacements arrive.
                    self.preview.status = PreviewStatus::Compiling;
                }
                CompileEvent::Failed(error) => self.set_compile_error(error),
                CompileEvent::Artifact(artifact) => {
                    self.preview.accept_artifact(artifact.key, artifact.pdf);
                    self.set_diagnostics(artifact.diagnostics);
                    self.preview.status = PreviewStatus::Ready(result.elapsed);
                    self.complete_pending_export();
                }
                CompileEvent::Rasterized { key, pages }
                    if self.preview.accepts_raster(key, self.document.revision) =>
                {
                    let pages = pages
                        .into_iter()
                        .enumerate()
                        .map(|(index, page)| {
                            make_preview_texture(context, key, index, page, self.preview.dark)
                        })
                        .collect();
                    self.preview.replace_raster(key, pages);
                }
                CompileEvent::RasterFailed { key, error }
                    if self.preview.accepts_raster(key, self.document.revision) =>
                {
                    self.preview.raster_error = Some(error.clone());
                    self.notice = Some(Notice {
                        message: format!(
                            "PDF is ready, but its raster preview is unavailable: {error}"
                        ),
                        kind: NoticeKind::Error,
                    });
                }
                CompileEvent::Rasterized { .. } | CompileEvent::RasterFailed { .. } => {}
            }
        }
    }

    fn receive_asset_results(&mut self, context: &egui::Context) {
        while let Some(result) = self.asset_loader.try_recv() {
            if result.token != self.asset_token || !self.document.kind.preview_only() {
                continue;
            }
            match result.output {
                Ok(LoadedAsset::Image(page)) => {
                    let key = ArtifactKey::unversioned(self.document.revision);
                    let image_size = page.size;
                    let mut texture = make_preview_texture(context, key, 0, page, false);
                    // Native PDF pages are 144-DPI rasters interpreted in
                    // 72-point coordinates. Doubling the image's logical
                    // raster size cancels that conversion and makes 100% mean
                    // one source image pixel per UI point.
                    texture.size = [
                        image_size[0].saturating_mul(2),
                        image_size[1].saturating_mul(2),
                    ];
                    self.preview.replace_asset(key, None, vec![texture]);
                }
                Ok(LoadedAsset::Pdf { bytes, pages }) => {
                    let key = ArtifactKey::unversioned(self.document.revision);
                    let pages = pages
                        .into_iter()
                        .enumerate()
                        .map(|(index, page)| {
                            make_preview_texture(context, key, index, page, self.preview.dark)
                        })
                        .collect();
                    self.preview.replace_asset(key, Some(bytes.into()), pages);
                    if let Some(page) = self.pending_asset_page.take() {
                        self.preview.requested_page =
                            Some(page.min(self.preview.pages.len().saturating_sub(1)));
                    }
                    self.complete_pending_export();
                }
                Err(error) => {
                    self.document_workflow.pending_export = None;
                    self.preview.status = PreviewStatus::Error;
                    self.notice = Some(Notice {
                        message: error,
                        kind: NoticeKind::Error,
                    });
                }
            }
        }
    }

    fn set_compile_error(&mut self, error: String) {
        self.preview.artifact_key = None;
        self.preview.status = PreviewStatus::Error;
        self.set_diagnostics(error);
    }

    fn set_diagnostics(&mut self, raw: String) {
        let display_name = self
            .preview_document_path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.document_name());
        let diagnostics = parse_typst_short_output(&raw, Some(Path::new(&display_name)));
        self.preview.set_diagnostics(raw, diagnostics);
    }

    fn schedule_compile_now(&mut self) {
        if !self.preview_processing_enabled() || !self.may_run_compilation() {
            self.compile_deadline = None;
            return;
        }
        self.compile_deadline = Some(Instant::now());
        self.preview.status = PreviewStatus::Waiting;
    }

    fn may_run_compilation(&self) -> bool {
        compilation_run_allowed(
            self.compilation_paused,
            self.document_workflow.pending_export.is_some(),
            self.captures.has_pending_for("main"),
        )
    }

    fn toggle_compilation_paused(&mut self) {
        self.compilation_paused = !self.compilation_paused;
        if self.compilation_paused {
            if !self.may_run_compilation() {
                self.compile_deadline = None;
                if let Err(error) = self.compiler.pause(self.document.revision) {
                    self.compilation_paused = false;
                    self.notice = Some(Notice {
                        message: format!("Could not pause the Typst watcher: {error}"),
                        kind: NoticeKind::Error,
                    });
                    return;
                }
            }
            let (message, kind) = compilation_notice(true);
            self.notice = Some(Notice {
                message: message.to_owned(),
                kind,
            });
        } else {
            if let Err(error) = self.sync_tinymist_change() {
                self.preview.tinymist_state = ServiceState::Degraded(error);
            }
            self.schedule_compile_now();
            let (message, kind) = compilation_notice(false);
            self.notice = Some(Notice {
                message: message.to_owned(),
                kind,
            });
        }
    }

    fn compile_pdf(&mut self, frame: &eframe::Frame) {
        if !self.typst_preview_available() {
            self.notice = Some(Notice {
                message: "Compile needs a Typst preview entry".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        let designated = self.designated_preview_path();
        let Some(path) = default_compile_pdf_path(
            designated.as_deref(),
            self.document.kind,
            self.document.path.as_deref(),
        ) else {
            // An unsaved Typst document has no meaningful adjacent output
            // path, so ask once where its first PDF should be written.
            self.choose_pdf_output(frame, PdfWriteIntent::Compile);
            return;
        };
        self.finish_pdf_output(path, PdfWriteIntent::Compile);
    }

    fn clear_preview_for_document(&mut self, preserve_designated_preview: bool) {
        self.manual_format_revision = None;
        self.external_file_change_notice = None;
        self.next_external_file_check = Instant::now();
        self.asset_token = self.asset_token.wrapping_add(1);
        self.asset_loader.cancel_before(self.asset_token);
        self.preview
            .clear_for_document(self.document.revision, preserve_designated_preview);
        self.pending_asset_page = None;
        self.editor_hover = None;
        if let Some(pending) = self.document_workflow.pending_export.take() {
            self.notice = Some(Notice {
                message: format!(
                    "Queued {} canceled because another document is now active",
                    pending.intent.dialog_title()
                ),
                kind: NoticeKind::Info,
            });
        }
    }

    fn handle_shortcuts(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        let refresh = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::R);
        let global_command = context.input_mut(|input| {
            consume_shortcut(input, |command| {
                matches!(
                    command_spec(command).menu,
                    CommandMenu::Application | CommandMenu::File | CommandMenu::View
                ) || matches!(
                    command,
                    AppCommand::Find
                        | AppCommand::FindReplace
                        | AppCommand::Format
                        | AppCommand::ToggleComment
                )
            })
        });
        if let Some(command) = global_command {
            self.execute_app_command(command, context, frame);
        }
        if context.input_mut(|input| input.consume_shortcut(&refresh)) {
            self.compile_pdf(frame);
        }
        let editor_id = source_editor_id(context);
        let source_focused = context.memory(|memory| memory.focused()) == Some(editor_id);
        if source_focused || !context.egui_wants_keyboard_input() {
            let history_command = context.input_mut(|input| {
                consume_shortcut(input, |command| {
                    matches!(command, AppCommand::Undo | AppCommand::Redo)
                })
            });
            if let Some(command) = history_command {
                self.execute_app_command(command, context, frame);
            }
        }
        let focus_tooltip = focus_tooltip_shortcut();
        if context.input_mut(|input| input.consume_shortcut(&focus_tooltip)) {
            let target = self
                .diagnostic_tooltip
                .as_ref()
                .map(|tooltip| (tooltip.origin, tooltip.detail.clone()))
                .or_else(|| {
                    context.data(|data| {
                        data.get_temp::<HoverTooltipOverlay>(native_hover_tooltip_id(context))
                            .map(|tooltip| (tooltip.origin, tooltip.detail.clone()))
                    })
                });
            if let Some((origin, detail)) = target {
                let interaction_id = tooltip_interaction_id(context);
                let identity = tooltip_identity(origin, &detail);
                context.data_mut(|data| {
                    let mut state = data
                        .get_temp::<TooltipInteractionState>(interaction_id)
                        .filter(|state| state.identity == identity)
                        .unwrap_or(TooltipInteractionState::new(identity));
                    state.focus_requested = true;
                    state.dismissed = false;
                    data.insert_temp(interaction_id, state);
                });
                context.request_repaint();
            }
        }
        if self.find_visible
            && context.input_mut(|input| input.consume_key(Modifiers::NONE, egui::Key::Escape))
        {
            self.find_visible = false;
            self.replace_visible = false;
            self.search.clear();
        }

        if self.view_mode.shows_preview() && !self.interactive_preview_active() {
            match context.input_mut(consume_preview_zoom_shortcut) {
                Some(PreviewZoomAction::In) => {
                    self.preview.requested_zoom = Some(
                        (self.preview.zoom * METRICS.preview.zoom_step)
                            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                    );
                    self.preview.fit_width = false;
                }
                Some(PreviewZoomAction::Out) => {
                    self.preview.requested_zoom = Some(
                        (self.preview.zoom / METRICS.preview.zoom_step)
                            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                    );
                    self.preview.fit_width = false;
                }
                Some(PreviewZoomAction::Reset) => {
                    self.preview.requested_zoom = Some(1.0);
                    self.preview.fit_width = false;
                }
                None => {}
            }
        }

        if let Some(delta) = context.input_mut(consume_ui_scale_shortcut) {
            self.adjust_ui_scale(delta, context);
        }
    }

    fn execute_app_command(
        &mut self,
        command: AppCommand,
        context: &egui::Context,
        frame: &eframe::Frame,
    ) {
        if command_spec(command).menu == CommandMenu::File && self.document_flow_busy() {
            self.notice = Some(Notice {
                message: "Finish the current file operation before starting another".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        match command {
            AppCommand::Settings => self.set_settings_visible(true),
            AppCommand::New => self.new_document(),
            AppCommand::NewWindow => {
                self.pending_window_requests
                    .push_back(EditorWindowRequest::New {
                        workspace_root: self.workspace_root.clone(),
                    });
            }
            AppCommand::Open => self.open_dialog(),
            AppCommand::OpenInNewWindow => self.open_in_new_window_dialog(frame),
            AppCommand::ChangeWorkspaceRoot => self.open_workspace_chooser(),
            AppCommand::Save => {
                if self.save_document(frame) {
                    self.request_format_after_manual_save();
                }
            }
            AppCommand::SaveAs => {
                self.save_as(frame);
            }
            AppCommand::ExportPdf => self.export_pdf(frame),
            AppCommand::Undo => self.undo_editor(context, false),
            AppCommand::Redo => self.undo_editor(context, true),
            AppCommand::Cut => self.cut_editor_selection(context),
            AppCommand::Copy => self.copy_editor_selection(context),
            AppCommand::Paste => self.request_editor_paste(context),
            AppCommand::SelectAll => self.select_all_editor(context),
            AppCommand::ToggleComment => self.toggle_comments(context),
            AppCommand::Find => self.toggle_find(),
            AppCommand::FindReplace => self.open_find(true),
            AppCommand::Format => self.request_format_document(),
            AppCommand::SyncPreview => {
                let cursor = self.editor_snapshot(context).cursor.primary.index.0;
                self.jump_source_to_preview(cursor);
            }
            AppCommand::Problems => self.problems_visible = !self.problems_visible,
            AppCommand::Explorer => self.filesystem_visible = !self.filesystem_visible,
            AppCommand::Code => self.view_mode = ViewMode::Code,
            AppCommand::Split => self.view_mode = ViewMode::Split,
            AppCommand::Preview => self.view_mode = ViewMode::Preview,
        }
    }

    fn process_native_menu_commands(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        while let Some(command) = self.queued_native_menu_commands.pop() {
            self.execute_app_command(command, context, frame);
        }
    }

    fn document_flow_busy(&self) -> bool {
        self.document_workflow.is_busy(
            self.rename_dialog.is_some(),
            self.pending_tool_picker.is_some(),
        )
    }

    fn set_settings_visible(&mut self, visible: bool) {
        self.settings_visible = visible;
        if visible {
            // A root popup and a child settings window must never compete for
            // native focus. Settings owns transient interaction until closed.
            self.app_popup = None;
            self.app_popup_had_focus = false;
            self.app_popup_blur_started = None;
        }
    }

    fn toggle_settings(&mut self) {
        self.set_settings_visible(!self.settings_visible);
    }

    fn open_app_popup(&mut self, popup: AppPopup) {
        self.app_popup_generation = self.app_popup_generation.wrapping_add(1);
        self.app_popup = Some(popup);
        self.app_popup_had_focus = false;
        self.app_popup_blur_started = None;
    }

    fn close_app_popup(&mut self) {
        self.app_popup = None;
        self.app_popup_had_focus = false;
        self.app_popup_blur_started = None;
    }

    fn open_find(&mut self, replace: bool) {
        self.find_visible = true;
        self.replace_visible |= replace;
        self.focus_find = true;
        if !self.view_mode.shows_code() {
            self.view_mode = ViewMode::Split;
        }
    }

    fn toggle_find(&mut self) {
        if self.find_visible {
            self.find_visible = false;
            self.replace_visible = false;
            self.search.clear();
        } else {
            self.open_find(false);
        }
    }

    fn adjust_ui_scale(&mut self, delta: i16, context: &egui::Context) {
        let mut edited = self
            .pending_settings
            .clone()
            .unwrap_or_else(|| self.settings.clone());
        let current = i16::try_from(edited.ui_scale_percent).unwrap_or(100);
        edited.ui_scale_percent = current.saturating_add(delta).clamp(75, 150) as u16;
        self.queue_settings(edited.clone(), context);
        apply_ui_scale(context, edited.ui_scale_percent);
        self.presentation.record_ui_scale(edited.ui_scale_percent);
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
        ) && self.preview.pages.is_empty()
        {
            self.captures.defer_target("main");
        }
        if let Some(status) = settled_snapshot_preview_status(scene, !self.preview.pages.is_empty())
        {
            self.preview.status = status;
        }
        let toolbar_anchor = Pos2::new(theme::SPACE.content, METRICS.chrome.toolbar_height);
        match scene {
            UiSnapshotScene::Main => {
                self.notice = None;
                self.preview.raw_diagnostics.clear();
                self.preview.diagnostics.clear();
                self.preview.tinymist_diagnostics.clear();
                self.mark_diagnostics_changed();
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
                if self.document_workflow.modal.is_none() {
                    self.document_workflow.modal = Some(AppModal::Unsaved {
                        message: format!(
                            "Save changes to {} before opening another file?",
                            self.document_name()
                        ),
                        pending: PendingDocumentAction {
                            action: DeferredDocumentAction::CloseWindow,
                            key: self.document.key(),
                            allow_discard: true,
                            description: "closing the document".to_owned(),
                        },
                    });
                }
            }
            UiSnapshotScene::AlertDialog => {
                if self.document_workflow.modal.is_none() {
                    self.document_workflow.modal = Some(AppModal::Alert {
                        title: "error".to_owned(),
                        message:
                            "The document could not be saved. Check the destination and try again."
                                .to_owned(),
                        kind: NoticeKind::Error,
                    });
                }
            }
            UiSnapshotScene::OverwriteDialog => {
                if self.document_workflow.modal.is_none() {
                    self.document_workflow.modal = Some(AppModal::Overwrite {
                        message: "This file changed on disk after it was opened. Overwrite it with the editor contents?"
                            .to_owned(),
                        path: self.document.path
                            .clone()
                            .unwrap_or_else(|| self.workspace_root.join("document.typ")),
                        key: self.document.key(),
                        expected_disk_fingerprint: Some(1),
                        observed_disk_fingerprint: Some(2),
                    });
                }
            }
            UiSnapshotScene::EditorContextMenu => {
                self.app_popup = Some(AppPopup::Editor {
                    anchor: Pos2::new(430.0, 250.0),
                    link: None,
                });
            }
            UiSnapshotScene::DocumentFontSelector => {
                self.document.source =
                    "#set text(font: \"Libertinus Serif\")\n= Font selector".to_owned();
                self.document.revision = self.document.revision.wrapping_add(1);
                self.prepare_editor_source_data();
                let font_char = self.document.source[..self.document.source.find("font").unwrap()]
                    .chars()
                    .count();
                let target = self
                    .editor_data
                    .font_argument_at(font_char)
                    .expect("snapshot font argument is valid");
                self.font_catalog = FontCatalog::snapshot_fixture();
                self.app_popup = Some(AppPopup::FontSelector {
                    anchor: Pos2::new(430.0, 250.0),
                    target,
                });
            }
            UiSnapshotScene::ExplorerContextMenu => {
                let path = self
                    .document
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
                self.preview.status = PreviewStatus::Ready(Duration::from_millis(18));
                self.recorded_status = Some(self.preview.status);
                self.notice = None;
                self.recorded_notice = None;
                self.status_log = VecDeque::from([
                    StatusLogEntry {
                        timestamp: "09:41:12Z".to_owned(),
                        detail: "PDF ready in 18 ms".to_owned(),
                        kind: NoticeKind::Success,
                    },
                    StatusLogEntry {
                        timestamp: "09:41:11Z".to_owned(),
                        detail: "Compiling document".to_owned(),
                        kind: NoticeKind::Info,
                    },
                    StatusLogEntry {
                        timestamp: "09:41:10Z".to_owned(),
                        detail: "PDF ready in 24 ms".to_owned(),
                        kind: NoticeKind::Success,
                    },
                    StatusLogEntry {
                        timestamp: "09:41:09Z".to_owned(),
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
                        .document
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
                self.preview.diagnostics = vec![
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
                self.preview.tinymist_diagnostics.clear();
                self.preview.raw_diagnostics.clear();
                self.mark_diagnostics_changed();
            }
            UiSnapshotScene::FindReplace => {
                self.notice = None;
                self.find_visible = true;
                self.replace_visible = true;
                self.find_query = "Typst".to_owned();
                self.replacement = "tiptoptyp".to_owned();
            }
            UiSnapshotScene::PreviewCompiling => {
                self.preview.status = PreviewStatus::Compiling;
            }
        }
    }

    /// Move a deterministic capture session to its next isolated UI state.
    /// Transient windows from the previous scene are closed before the new
    /// scene is painted, while the loaded fixture and rendered preview remain
    /// available across the whole process.
    pub(crate) fn set_capture_step(&mut self, step: &UiCaptureStep, context: &egui::Context) {
        let capture_target_changed = self
            .snapshot_scene
            .is_some_and(|scene| scene.viewport_target() != step.scene.viewport_target());
        self.settings_visible = false;
        self.typst_overrides_visible = false;
        self.workspace_chooser_visible = false;
        self.problems_visible = false;
        self.find_visible = false;
        self.replace_visible = false;
        self.search.clear();
        self.focus_find = false;
        self.diagnostic_tooltip = None;
        self.close_app_popup();
        self.document_workflow.clear_modal();
        self.rename_dialog = None;
        self.rename_overlay_had_focus = false;
        self.rename_overlay_suspended = false;
        self.staged_ui_font_weight = None;
        self.staged_code_font_weight = None;
        self.notice = None;
        self.preview.raw_diagnostics.clear();
        self.preview.diagnostics.clear();
        self.preview.tinymist_diagnostics.clear();
        self.mark_diagnostics_changed();
        self.status_log.clear();
        self.recorded_status = None;
        self.recorded_notice = None;
        self.preview.status = PreviewStatus::Ready(Duration::ZERO);
        self.document.restore_saved_source();
        self.theme_override = Some(step.theme.clone());
        self.snapshot_scene = Some(step.scene);

        if capture_target_changed {
            // Immediate child viewports have their own renderer texture state.
            // Refresh the shared definitions only when crossing that boundary,
            // rather than rebuilding every configured font for every image.
            self.presentation.invalidate_fonts();
        }

        if matches!(
            step.scene,
            UiSnapshotScene::Main | UiSnapshotScene::ProblemsPanel | UiSnapshotScene::FindReplace
        ) && self.preview.pages.is_empty()
        {
            self.schedule_compile_now();
        }
        context.request_repaint();
    }

    fn sync_runtime_settings(&mut self, context: &egui::Context) {
        let theme_request = active_theme_request(
            &self.settings,
            context.system_theme(),
            self.theme_override.as_ref(),
        );
        let request = ResolvedPresentationRequest::resolve(
            &self.settings,
            theme_request,
            self.font_catalog_revision,
        );
        let changes = self.presentation.changes(&request);
        self.preview
            .set_requested_backend(request.preview_preference);

        if changes.ui_scale {
            apply_ui_scale(context, request.ui_scale_percent);
        }
        if changes.fonts {
            let ui_family = self
                .selected_font_family(
                    request.ui_font.path.as_deref(),
                    request.ui_font.family.as_deref(),
                )
                .cloned();
            let code_family = self
                .selected_font_family(
                    request.code_font.path.as_deref(),
                    request.code_font.family.as_deref(),
                )
                .cloned();
            self.font_configuration = theme::configure_editor_fonts(
                context,
                theme::FontRequest {
                    family: ui_family.as_ref(),
                    fallback_path: request.ui_font.path.as_deref().map(Path::new),
                    fallback_face_index: request.ui_font.face_index,
                },
                theme::FontRequest {
                    family: code_family.as_ref(),
                    fallback_path: request.code_font.path.as_deref().map(Path::new),
                    fallback_face_index: request.code_font.face_index,
                },
                request.ui_font.monospace,
                request.ui_font.weight,
                request.code_font.weight,
            );
            if request.ui_font.path.is_some() && !self.font_configuration.custom_ui_loaded {
                self.notice = Some(Notice {
                    message: "The selected UI font is unavailable; using a fallback".to_owned(),
                    kind: NoticeKind::Error,
                });
            }
            if request.code_font.path.is_some() && !self.font_configuration.custom_editor_loaded {
                self.notice = Some(Notice {
                    message: "The selected code font is unavailable; using a fallback".to_owned(),
                    kind: NoticeKind::Error,
                });
            }
            theme::configure_ui_font(context, self.font_configuration.weighted_ui_loaded);
        }
        if changes.theme {
            self.reload_active_theme(context, &request.theme);
        }
        if changes.typst_overrides
            && !changes.theme
            && let Some(active) = &self.imported_theme
        {
            let styles = ResolvedTypstStyles::resolve(
                theme::syntax_palette(active.dark_mode),
                Some(&active.syntect_theme),
                request.typst_overrides.for_dark(active.dark_mode),
            );
            self.highlighter.set_styles(styles);
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
        let preview_appearance_changed = next_preview_dark != self.preview.dark;
        if preview_appearance_changed {
            self.preview.dark = next_preview_dark;
            self.rebuild_preview_textures(context);
        }
        let refresh_tools = std::mem::take(&mut self.tool_refresh_requested);
        if refresh_tools || changes.typst {
            let next_typst = resolve_tool(ToolKind::Typst, &request.typst);
            let typst_program_changed = next_typst.program != self.typst_tool.program;
            self.typst_tool = next_typst;
            if refresh_tools || typst_program_changed {
                self.schedule_compile_now();
            }
        }

        let mut tinymist_program_changed = false;
        if refresh_tools || changes.tinymist {
            let next_tinymist = resolve_tool(ToolKind::Tinymist, &request.tinymist);
            tinymist_program_changed = next_tinymist.program != self.tinymist_tool.program;
            self.tinymist_tool = next_tinymist;
        }

        // A system appearance event changes the effective preview palette when
        // the document follows the interface, so refresh Tinymist as well as
        // the raster-page textures.
        if tinymist_restart_required(
            changes.document_theme,
            preview_appearance_changed,
            changes.preview_preference,
            tinymist_program_changed,
            refresh_tools,
        ) {
            self.restart_tinymist();
            self.schedule_compile_now();
        }
        self.presentation.commit(request);
    }

    fn reload_active_theme(&mut self, context: &egui::Context, request: &ActiveThemeRequest) {
        let (active, error) = load_active_theme_or_fallback(request);
        theme::set_imported_palette(Some((active.dark_mode, active.palette)));
        theme::configure_styles(context);
        self.generic_highlighter
            .set_custom_theme(Some(active.syntect_theme.clone()));
        self.highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::syntax_palette(active.dark_mode),
            Some(&active.syntect_theme),
            self.settings.typst_overrides.for_dark(active.dark_mode),
        ));
        context.set_theme(active_theme_preference(
            self.settings.interface_theme,
            self.theme_override.is_some(),
            active.dark_mode,
        ));
        let name = active.name.as_deref().unwrap_or("Color theme");
        self.notice = if self.snapshot_scene.is_some() {
            None
        } else {
            Some(match error {
                Some(error) => Notice {
                    message: format!("{error}; using {name}"),
                    kind: NoticeKind::Error,
                },
                None => Notice {
                    message: format!("Applied {name}"),
                    kind: NoticeKind::Success,
                },
            })
        };
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

    fn process_open_requests(&mut self, context: &egui::Context) {
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
        if close_requested && self.is_dirty() && !self.document_workflow.allow_close {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.document_workflow.modal.is_none() {
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
        self.document.replace_untitled(DEFAULT_SOURCE);
        self.autosave_deadline = None;
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.clear_preview_for_document(false);
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
        if self.document_workflow.pending_dialog.is_some() {
            return;
        }
        let Some(dialog) = self.native_file_dialog(frame) else {
            return;
        };
        let mut dialog = dialog.set_title("Open project folder");
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.document_workflow.pending_dialog = Some(PendingDialog::new(
            DocumentDialogRequest {
                target: DocumentDialogTarget::OpenFolder,
                key: self.document.key(),
            },
            dialog.pick_folder(),
        ));
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
            .document
            .path
            .as_ref()
            .is_some_and(|path| path.starts_with(&root))
        {
            if let Some(path) = self.document.path.clone() {
                self.remember_open_document(&path);
            }
            self.reset_document_services();
            if self.preview_processing_enabled() {
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
        if self.document_workflow.pending_dialog.is_some() {
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
        self.document_workflow.pending_dialog = Some(PendingDialog::new(
            DocumentDialogRequest {
                target: DocumentDialogTarget::OpenFileInNewWindow,
                key: self.document.key(),
            },
            dialog.pick_file(),
        ));
    }

    fn start_open_dialog(&mut self, frame: &eframe::Frame) {
        if self.document_workflow.pending_dialog.is_some() {
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
        self.document_workflow.pending_dialog = Some(PendingDialog::new(
            DocumentDialogRequest {
                target: DocumentDialogTarget::OpenFile,
                key: self.document.key(),
            },
            dialog.pick_file(),
        ));
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
        let keep_designated_preview = self.should_keep_designated_preview(&path);
        let workspace_root_changed = !path.starts_with(&self.workspace_root);
        let preserve_workspace_snapshot = preserve_workspace_snapshot_for_open(
            self.workspace.is_some(),
            &self.workspace_root,
            &path,
        );
        if workspace_root_changed && let Some(parent) = path.parent() {
            self.workspace_root = canonical_or_absolute(&discover_project_root(parent));
        }
        let workspace_root = self.workspace_root.clone();
        self.remember_workspace(&workspace_root);

        self.document.replace_loaded(
            source,
            path.clone(),
            kind,
            kind.is_editable().then(|| fingerprint(&bytes)),
        );
        self.autosave_deadline = None;
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.remember_open_document(&path);
        self.clear_preview_for_document(keep_designated_preview);
        self.search.clear();
        if !preserve_workspace_snapshot {
            self.reset_document_services();
        } else if keep_designated_preview {
            if self.tinymist_generation.is_some() {
                self.reopen_tinymist_current_document(&path, kind);
            } else {
                self.restart_tinymist();
            }
        } else {
            self.restart_tinymist();
        }

        if self.preview_processing_enabled() {
            self.schedule_compile_now();
        } else if kind.preview_only() {
            self.preview.status = PreviewStatus::Compiling;
            if let Err(error) = self.asset_loader.request(self.asset_token, path, kind) {
                self.preview.status = PreviewStatus::Error;
                self.notice = Some(Notice {
                    message: error,
                    kind: NoticeKind::Error,
                });
            }
        } else {
            self.preview.status = PreviewStatus::Ready(Duration::ZERO);
        }
        true
    }

    fn save_document(&mut self, frame: &eframe::Frame) -> bool {
        if !self.document.kind.is_editable() {
            return true;
        }
        if let Some(path) = self.document.path.clone() {
            self.save_to(path)
        } else {
            self.save_as(frame)
        }
    }

    fn save_as(&mut self, frame: &eframe::Frame) -> bool {
        if !self.document.kind.is_editable() {
            return false;
        }
        if self.document_workflow.pending_dialog.is_some() {
            return false;
        }
        let typst = self.document.kind.is_typst();
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
        self.document_workflow.pending_dialog = Some(PendingDialog::new(
            DocumentDialogRequest {
                target: DocumentDialogTarget::SaveAs { typst },
                key: self.document.key(),
            },
            dialog.save_file(),
        ));
        false
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        self.save_to_with_intent(path, SaveIntent::Explicit)
    }

    fn save_to_with_intent(&mut self, path: PathBuf, intent: SaveIntent) -> bool {
        let path_changed = self
            .document
            .path
            .as_ref()
            .is_none_or(|current| !same_path(current, &path));
        if !path_changed {
            match intent {
                SaveIntent::Explicit if !self.confirm_disk_unchanged(&path) => return false,
                SaveIntent::ExplicitConfirmed => {}
                SaveIntent::Auto
                    if !disk_matches_fingerprint(&path, self.document.disk_fingerprint) =>
                {
                    self.notice = Some(Notice {
                        message: "Auto-save paused because the file changed on disk".to_owned(),
                        kind: NoticeKind::Error,
                    });
                    return false;
                }
                _ => {}
            }
        }
        match atomic_write(&path, self.document.source.as_bytes()) {
            Ok(()) => {
                let path = path.canonicalize().unwrap_or(path);
                if !path.starts_with(&self.workspace_root)
                    && let Some(parent) = path.parent()
                {
                    self.workspace_root = canonical_or_absolute(&discover_project_root(parent));
                }
                let saved_kind = if path_changed {
                    DocumentKind::detect(&path, self.document.source.as_bytes())
                        .ok()
                        .filter(|kind| kind.is_editable())
                        .unwrap_or(DocumentKind::Text)
                } else {
                    self.document.kind
                };
                self.document.complete_save(
                    path.clone(),
                    saved_kind,
                    fingerprint(self.document.source.as_bytes()),
                    path_changed,
                );
                self.external_file_change_notice = None;
                if path_changed {
                    self.preview.artifact_key = None;
                    self.reset_document_services();
                    self.schedule_compile_now();
                } else {
                    self.refresh_workspace();
                    if self.preview_processing_enabled() {
                        // Any saved project file may be imported or read by
                        // the designated entry, so refresh the CLI fallback.
                        self.schedule_compile_now();
                    }
                }
                self.autosave_deadline = None;
                if let Some(path) = self.document.path.clone() {
                    self.remember_open_document(&path);
                }
                self.schedule_project_index();
                if let Some(mut action) = self.document_workflow.post_save_action.take() {
                    action.key = self.document.key();
                    action.allow_discard = false;
                    self.document_workflow.pending_action = Some(action);
                }
                true
            }
            Err(error) => {
                match intent {
                    SaveIntent::Explicit | SaveIntent::ExplicitConfirmed => {
                        self.document_workflow.post_save_action = None;
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
        let Some(expected) = self.document.disk_fingerprint else {
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
                self.document_workflow.post_save_action = None;
                self.show_file_error(format!(
                    "Could not check {} before saving: {error}",
                    path.display()
                ));
                return false;
            }
        };

        self.document_workflow.modal = Some(AppModal::Overwrite {
            message: description,
            path: path.to_path_buf(),
            key: self.document.key(),
            expected_disk_fingerprint: self.document.disk_fingerprint,
            observed_disk_fingerprint,
        });
        self.document_workflow.modal_had_focus = false;
        self.document_workflow.modal_suspended = false;
        false
    }

    fn export_pdf(&mut self, frame: &eframe::Frame) {
        self.choose_pdf_output(frame, PdfWriteIntent::Export);
    }

    fn choose_pdf_output(&mut self, frame: &eframe::Frame, intent: PdfWriteIntent) {
        if !self.typst_preview_available() && self.document.kind != DocumentKind::Pdf {
            self.notice = Some(Notice {
                message: format!(
                    "{} needs a Typst preview entry or PDF document",
                    intent.dialog_title()
                ),
                kind: NoticeKind::Info,
            });
            return;
        }
        if let Some(pending) = self.document_workflow.pending_export.as_ref() {
            self.notice = Some(Notice {
                message: format!("{} is already queued", pending.intent.dialog_title()),
                kind: NoticeKind::Info,
            });
            return;
        }
        if self.document_workflow.pending_export_dialog.is_some() {
            return;
        }
        let export_source = if self.typst_preview_available() {
            Some(self.preview_document_path())
        } else {
            self.document.path.clone()
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
            .set_title(intent.dialog_title())
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
        self.document_workflow.pending_export_dialog = Some(PendingDialog::new(
            ExportDialogRequest {
                document_epoch: self.document.epoch,
                intent,
            },
            dialog.save_file(),
        ));
    }

    fn poll_export_dialog(&mut self, context: &egui::Context) {
        let DialogPoll::Ready { request, selection } =
            poll_dialog(&mut self.document_workflow.pending_export_dialog, context)
        else {
            return;
        };
        if let Some(file) = selection {
            if self.document.epoch == request.document_epoch {
                self.finish_pdf_output(file.path().to_path_buf(), request.intent);
            } else {
                self.notice = Some(Notice {
                    message: format!(
                        "{} canceled because another document is now active",
                        request.intent.dialog_title()
                    ),
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
            ToolPickerTarget::UiFont => "Choose UI font",
            ToolPickerTarget::CodeFont => "Choose code font",
        };
        let Some(dialog) = self.settings_file_dialog(frame) else {
            return;
        };
        let mut dialog = dialog.set_title(title);
        if matches!(target, ToolPickerTarget::SublimeTheme { .. }) {
            dialog = dialog.add_filter("Sublime themes", &["tmTheme", "sublime-color-scheme"]);
        }
        if matches!(
            target,
            ToolPickerTarget::UiFont | ToolPickerTarget::CodeFont
        ) {
            dialog = dialog.add_filter("Font files", &["ttf", "otf", "ttc", "otc"]);
        }
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.pending_tool_picker = Some(PendingDialog::new(target, dialog.pick_file()));
        settings_context.request_repaint();
    }

    fn poll_tool_picker(&mut self, context: &egui::Context) {
        let DialogPoll::Ready {
            request: target,
            selection,
        } = poll_dialog(&mut self.pending_tool_picker, context)
        else {
            return;
        };
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
            ToolPickerTarget::UiFont => {
                if let Err(error) = theme::load_ui_font_bytes(file.path()) {
                    self.notice = Some(Notice {
                        message: error,
                        kind: NoticeKind::Error,
                    });
                    self.pending_settings = Some(edited);
                    context.request_repaint();
                    return;
                }
                edited.ui_font_path = Some(value);
                edited.ui_font_family = None;
                edited.ui_font_face_index = 0;
                edited.ui_font_monospace = false;
                self.notice = Some(Notice {
                    message: "UI font selected; applying it now".to_owned(),
                    kind: NoticeKind::Success,
                });
            }
            ToolPickerTarget::CodeFont => {
                if let Err(error) = theme::load_ui_font_bytes(file.path()) {
                    self.notice = Some(Notice {
                        message: error,
                        kind: NoticeKind::Error,
                    });
                    self.pending_settings = Some(edited);
                    context.request_repaint();
                    return;
                }
                edited.code_font_path = Some(value);
                edited.code_font_family = None;
                edited.code_font_face_index = 0;
                self.notice = Some(Notice {
                    message: "Code font selected; applying it now".to_owned(),
                    kind: NoticeKind::Success,
                });
            }
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
        let DialogPoll::Ready { request, selection } =
            poll_dialog(&mut self.document_workflow.pending_dialog, context)
        else {
            return;
        };
        let target = request.target;
        let key = request.key;
        let Some(file) = selection else {
            self.document_workflow.post_save_action = None;
            self.schedule_autosave_if_needed();
            return;
        };
        let mut path = file.path().to_path_buf();
        match target {
            DocumentDialogTarget::OpenFile => {
                if self.document.epoch != key.epoch {
                    self.notice = Some(Notice {
                        message: "Open canceled because another document is now active".to_owned(),
                        kind: NoticeKind::Info,
                    });
                } else if self.document.revision != key.revision {
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
                if self.document.epoch != key.epoch {
                    self.notice = Some(Notice {
                        message: "Open Folder canceled because another document is now active"
                            .to_owned(),
                        kind: NoticeKind::Info,
                    });
                } else if self.document.revision != key.revision {
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
                if self.document.epoch != key.epoch {
                    self.document_workflow.post_save_action = None;
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

    fn finish_pdf_output(&mut self, mut path: PathBuf, intent: PdfWriteIntent) {
        if let Some(pending) = self.document_workflow.pending_export.as_ref() {
            self.notice = Some(Notice {
                message: format!("{} is already queued", pending.intent.dialog_title()),
                kind: NoticeKind::Info,
            });
            return;
        }
        if path.extension().is_none() {
            path.set_extension("pdf");
        }

        self.document_workflow.pending_export = Some(PendingExport {
            path,
            document_epoch: self.document.epoch,
            intent,
        });
        if self
            .preview
            .artifact_key
            .is_some_and(|key| key.revision == self.document.revision)
            && self.preview.pdf.is_some()
        {
            self.complete_pending_export();
            return;
        }
        self.notice = Some(Notice {
            message: intent.queued_message().to_owned(),
            kind: NoticeKind::Info,
        });
        if self.preview_processing_enabled() {
            self.schedule_compile_now();
        }
    }

    fn complete_pending_export(&mut self) {
        let (Some(pending), Some(pdf)) = (
            take_matching_export(
                &mut self.document_workflow.pending_export,
                self.document.epoch,
            ),
            self.preview.pdf.as_deref(),
        ) else {
            return;
        };
        if let Err(error) = atomic_write(&pending.path, pdf) {
            self.show_file_error(error);
        } else {
            self.notice = Some(Notice {
                message: format!(
                    "{} {}",
                    pending.intent.completed_verb(),
                    pending.path.display()
                ),
                kind: NoticeKind::Success,
            });
        }
        if self.compilation_paused && !self.may_run_compilation() {
            self.compile_deadline = None;
            let _ = self.compiler.pause(self.document.revision);
        }
    }

    fn request_document_replacement(&mut self, action: DeferredDocumentAction, description: &str) {
        if self.document_flow_busy() {
            return;
        }
        let key = self.document.key();
        let dirty = self.document.is_dirty();
        let name = self.document.name();
        self.document_workflow
            .queue_replacement(key, dirty, &name, action, description);
    }

    fn present_unsaved_prompt(&mut self, pending: PendingDocumentAction) {
        let key = self.document.key();
        let name = self.document.name();
        self.document_workflow.present_unsaved(key, &name, pending);
    }

    fn execute_pending_document_action(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        let Some(mut pending) = self.document_workflow.pending_action.take() else {
            return;
        };
        if pending.key.epoch != self.document.epoch {
            self.document_workflow.post_save_action = None;
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
                self.document_workflow.post_save_action = Some(*action);
                if self.save_document(frame)
                    && let Some(mut action) = self.document_workflow.post_save_action.take()
                {
                    action.key = self.document.key();
                    action.allow_discard = false;
                    self.document_workflow.pending_action = Some(action);
                }
                return;
            }
            DeferredDocumentAction::ForceSave {
                path,
                key,
                expected_disk_fingerprint,
                observed_disk_fingerprint,
            } => {
                let same_document = key.epoch == self.document.epoch
                    && self
                        .document
                        .path
                        .as_ref()
                        .is_some_and(|current| same_path(current, &path))
                    && self.document.disk_fingerprint == expected_disk_fingerprint;
                if !same_document {
                    self.document_workflow.post_save_action = None;
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
                        self.document_workflow.post_save_action = None;
                        self.show_file_error(format!(
                            "Could not recheck {} before saving: {error}",
                            path.display()
                        ));
                        return;
                    }
                };
                let continue_after_save = self.document_workflow.post_save_action.is_some();
                let saved = if key.revision != self.document.revision
                    || current_disk_fingerprint != observed_disk_fingerprint
                {
                    self.save_to_with_intent(path, SaveIntent::Explicit)
                } else {
                    self.save_to_with_intent(path, SaveIntent::ExplicitConfirmed)
                };
                if saved && !continue_after_save {
                    self.request_format_after_manual_save();
                }
                return;
            }
            action => pending.action = action,
        }

        if pending.allow_discard {
            if pending.key.revision != self.document.revision {
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
                self.document_workflow.allow_close = true;
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
        self.document_workflow.present_error(message);
    }

    fn reset_document_services(&mut self) {
        let root = self.project_root();
        self.workspace = None;
        self.workspace_error = None;
        self.request_workspace_scan(root);
        self.next_workspace_refresh = Instant::now() + WORKSPACE_REFRESH_INTERVAL;
        self.restart_tinymist();
    }

    fn should_keep_designated_preview(&self, path: &Path) -> bool {
        self.designated_preview_path()
            .is_some_and(|_| path.starts_with(&self.workspace_root))
    }

    /// Switch the active editor document without tearing down the designated
    /// preview document or its native web view. A non-Typst child file has no
    /// current LSP document, but the preview entry remains open and visible in
    /// Split or Preview mode.
    fn reopen_tinymist_current_document(&mut self, path: &Path, kind: DocumentKind) {
        let Some(generation) = self.tinymist_generation else {
            return;
        };
        let old_uri = self.tinymist_uri.take();
        let preview_uri = self.tinymist_preview_uri.clone();
        let document = if kind.is_typst() {
            match TextDocument::from_path(
                path,
                revision_as_i32(self.document.revision),
                &self.document.source,
            ) {
                Ok(document) => Some(document),
                Err(error) => {
                    self.preview.tinymist_state = ServiceState::Degraded(error.to_string());
                    return;
                }
            }
        } else {
            None
        };
        if self.tinymist_current_open
            && let Some(uri) = old_uri
            && preview_uri.as_deref() != Some(uri.as_str())
        {
            let _ = self.tinymist.did_close(generation, uri);
        }
        self.tinymist_current_open = false;
        if !kind.is_typst() {
            return;
        }
        let document = document.expect("Typst documents have a Tinymist document");
        self.tinymist_uri = Some(document.uri.clone());
        if preview_uri.as_deref() == Some(document.uri.as_str()) {
            self.tinymist_current_open = true;
            if let Err(error) = self.tinymist.did_change(
                generation,
                document.uri,
                revision_as_i32(self.document.revision),
                self.document.source.clone(),
            ) {
                self.preview.tinymist_state = ServiceState::Degraded(error.to_string());
            }
            return;
        }
        if self.preview.tinymist_lsp_ready {
            match self.tinymist.did_open(generation, document) {
                Ok(()) => self.tinymist_current_open = true,
                Err(error) => {
                    self.preview.tinymist_state = ServiceState::Degraded(error.to_string())
                }
            }
        }
    }

    fn request_workspace_scan(&mut self, root: PathBuf) {
        if let Err(error) = self
            .workspace_scan
            .start("tiptoptyp-workspace-scan", move || {
                WorkspaceSnapshot::scan(&root).map_err(|error| error.to_string())
            })
        {
            self.workspace_error = Some(format!("Could not start workspace scan: {error}"));
        }
    }

    fn poll_workspace_scan(&mut self, context: &egui::Context) {
        match self.workspace_scan.poll() {
            LatestJobPoll::Idle | LatestJobPoll::Pending => {}
            LatestJobPoll::Ready(snapshot) => {
                let font_files = workspace_snapshot_font_files(&snapshot);
                let should_rescan_fonts = !self.font_catalog_scan.is_running()
                    && snapshot.root == self.font_catalog_root
                    && !self.font_catalog.workspace_files_match(&font_files);
                if let Some(workspace) = &mut self.workspace {
                    workspace.apply_snapshot(snapshot);
                } else {
                    self.workspace = Some(WorkspaceTree::from_snapshot(snapshot));
                }
                self.workspace_error = None;
                if should_rescan_fonts {
                    self.request_font_catalog_scan(context);
                }
            }
            LatestJobPoll::Failed(error) => {
                self.workspace_error = Some(format!("Could not scan workspace: {error}"));
            }
        }
    }

    fn refresh_workspace(&mut self) {
        if !self.workspace_scan.is_running() {
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
        self.poll_workspace_scan(context);
        self.poll_external_file_change(context);
        if self.workspace_scan.is_running() {
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

    fn poll_external_file_change(&mut self, context: &egui::Context) {
        let now = Instant::now();
        if now < self.next_external_file_check {
            context.request_repaint_after(self.next_external_file_check - now);
            return;
        }
        self.next_external_file_check = now + EXTERNAL_FILE_CHECK_INTERVAL;
        context.request_repaint_after(EXTERNAL_FILE_CHECK_INTERVAL);
        let Some(path) = self.document.path.clone() else {
            return;
        };
        if !self.document.kind.is_editable() {
            return;
        }
        let observed = match fs::read(&path) {
            Ok(contents) => ExternalFileObservation::Present(fingerprint(&contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ExternalFileObservation::Missing
            }
            Err(_) => return,
        };
        if matches!(observed, ExternalFileObservation::Present(fingerprint) if Some(fingerprint) == self.document.disk_fingerprint)
        {
            self.external_file_change_notice = None;
            return;
        }
        if self.external_file_change_notice == Some(observed) {
            return;
        }
        self.external_file_change_notice = Some(observed);
        let message = match (observed, self.is_dirty()) {
            (ExternalFileObservation::Missing, true) => format!(
                "{} was deleted outside tiptoptyp; save or discard your edits before recreating it",
                path.display()
            ),
            (ExternalFileObservation::Missing, false) => format!(
                "{} was deleted outside tiptoptyp; save to recreate it or open another file",
                path.display()
            ),
            (ExternalFileObservation::Present(_), true) => format!(
                "{} changed on disk; save or discard your edits before reloading",
                path.display()
            ),
            (ExternalFileObservation::Present(_), false) => {
                if self.load_path(path.clone()) {
                    self.notice = Some(Notice {
                        message: format!("Reloaded {} after an external change", path.display()),
                        kind: NoticeKind::Info,
                    });
                    context.request_repaint();
                }
                return;
            }
        };
        self.notice = Some(Notice {
            message,
            kind: NoticeKind::Error,
        });
        context.request_repaint();
    }

    fn tinymist_document_path(&self) -> PathBuf {
        self.tinymist_unsaved_document
            .as_ref()
            .map(|document| document.path().to_owned())
            .or_else(|| self.document.path.clone())
            .unwrap_or_else(|| self.preview_document_path())
    }

    fn restart_tinymist(&mut self) {
        self.restart_tinymist_with_handoff(false);
    }

    fn restart_tinymist_preserving_preview(&mut self) {
        self.restart_tinymist_with_handoff(true);
    }

    fn restart_tinymist_with_handoff(&mut self, preserve_preview: bool) {
        // A restart invalidates every outstanding formatting request. Do not
        // carry save-after-format intent into the replacement generation.
        self.manual_format_revision = None;
        let start_preview = self.tinymist_session_requested();
        self.preview.tinymist_preview_enabled = start_preview;
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
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let has_webview = self.webview.is_some();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let has_webview = false;
        let retain_preview_surface = retain_preview_surface_for_restart(
            preserve_preview,
            start_preview,
            self.preview.interactive_url.is_some(),
            has_webview,
        );
        if !retain_preview_surface {
            self.preview.interactive_url = None;
        }
        self.preview.tinymist_lsp_ready = false;
        self.preview.tinymist_diagnostics.clear();
        self.mark_diagnostics_changed();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.webview_reload_pending = false;
            if !retain_preview_surface {
                self.webview = None;
                self.webview_url = None;
                self.webview_navigation = None;
            }
        }
        if !self.document.kind.is_typst() && self.designated_preview_path().is_none() {
            self.preview.tinymist_state =
                ServiceState::Disabled("The selected file is not a Typst document".to_owned());
            self.preview.webview_state =
                ServiceState::Disabled("Binary files use the native preview".to_owned());
            return;
        }
        if !self.tinymist_tool.is_available() {
            let reason = self
                .tinymist_tool
                .fallback_reason
                .clone()
                .unwrap_or_else(|| "Tinymist is unavailable".to_owned());
            self.preview.tinymist_state = ServiceState::Failed(reason.clone());
            self.preview.webview_state = ServiceState::Failed(reason);
            return;
        }
        self.preview.tinymist_state = ServiceState::Starting("Launching Tinymist LSP".to_owned());
        self.preview.webview_state = if self.preview.requested_backend == PreviewPreference::Native
        {
            ServiceState::Disabled("Interactive preview is not requested".to_owned())
        } else if !cfg!(any(target_os = "macos", target_os = "windows")) {
            ServiceState::Unsupported(
                "Embedded Tinymist preview is currently available on macOS and Windows".to_owned(),
            )
        } else if !start_preview {
            ServiceState::Disabled(
                "Interactive preview is paused while Code view is active".to_owned(),
            )
        } else if retain_preview_surface {
            ServiceState::Ready(
                "Keeping the current preview visible while the pinned entry starts".to_owned(),
            )
        } else {
            ServiceState::Starting("Waiting for Tinymist's preview server".to_owned())
        };
        let mut config = TinymistConfig::new(self.project_root())
            .with_executable(self.tinymist_tool.program.clone())
            .with_font_paths(self.font_catalog.workspace_directories());
        if let Some(entry_path) = self.designated_preview_path() {
            config = config.with_entry_path(entry_path);
        }
        config.start_preview = start_preview;
        config.preview.invert_colors =
            tinymist_invert_colors(self.settings.document_theme, self.preview.dark);
        match self.tinymist.start_workspace(config) {
            Ok(generation) => {
                let version = revision_as_i32(self.document.revision);
                let current_document = if self.document.kind.is_typst() {
                    if let Some(path) = self.document.path.as_deref() {
                        TextDocument::from_path(path, version, self.document.source.clone())
                            .map_err(|error| error.to_string())
                    } else {
                        let source_dir = self
                            .current_directory()
                            .unwrap_or_else(|| self.project_root());
                        match UnsavedTextDocument::create(
                            self.project_root(),
                            source_dir,
                            self.document_name(),
                            &self.document.source,
                        ) {
                            Ok(document) => {
                                let text_document =
                                    document.text_document(version, self.document.source.clone());
                                self.tinymist_unsaved_document = Some(document);
                                Ok(text_document)
                            }
                            Err(error) => Err(error.to_string()),
                        }
                    }
                } else {
                    self.preview_document_source().and_then(|source| {
                        let path = self.preview_document_path();
                        TextDocument::from_path(&path, version, source)
                            .map_err(|error| error.to_string())
                    })
                };
                let current_document = match current_document {
                    Ok(document) => document,
                    Err(error) => {
                        let _ = self.tinymist.stop_workspace(generation);
                        self.preview.tinymist_state = ServiceState::Failed(error.to_string());
                        return;
                    }
                };

                let preview_document =
                    if !self.document.kind.is_typst() || self.current_is_preview_document() {
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
                                self.preview.tinymist_state = ServiceState::Failed(error);
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
                    self.preview.tinymist_state = ServiceState::Failed(error.to_string());
                }
            }
            Err(error) => {
                self.preview.interactive_url = None;
                self.preview.tinymist_state = ServiceState::Failed(error.to_string());
                self.preview.webview_state =
                    ServiceState::Failed("Could not restart the pinned preview".to_owned());
            }
        }
    }

    fn sync_tinymist_change(&self) -> Result<(), String> {
        if !self.document.kind.is_typst() {
            return Ok(());
        }
        if let Some(document) = &self.tinymist_unsaved_document {
            document
                .update_backing_source(&self.document.source)
                .map_err(|error| error.to_string())?;
        }
        let (Some(generation), Some(uri)) = (self.tinymist_generation, self.tinymist_uri.clone())
        else {
            return Ok(());
        };
        if !self.tinymist_current_open {
            // Initialized opens this document using the newest buffer.
            return Ok(());
        }
        self.tinymist
            .did_change(
                generation,
                uri,
                revision_as_i32(self.document.revision),
                self.document.source.clone(),
            )
            .map_err(|error| error.to_string())
    }

    fn receive_tinymist_events(&mut self, context: &egui::Context) {
        while let Some(event) = self.tinymist.try_recv() {
            match event {
                TinymistEvent::Starting { .. } => {
                    self.preview.tinymist_lsp_ready = false;
                    self.preview.tinymist_state =
                        ServiceState::Starting("Launching Tinymist LSP".to_owned());
                }
                TinymistEvent::Initialized { generation } => {
                    self.preview.tinymist_lsp_ready = true;
                    self.preview.tinymist_state = if self.preview.tinymist_preview_enabled {
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
                            revision_as_i32(self.document.revision),
                            self.document.source.clone(),
                        );
                        match self.tinymist.did_open(generation, document) {
                            Ok(()) => self.tinymist_current_open = true,
                            Err(error) => {
                                self.preview.tinymist_state =
                                    ServiceState::Degraded(error.to_string())
                            }
                        }
                    }
                }
                TinymistEvent::PreviewReady { url, .. } => {
                    self.preview.tinymist_state =
                        ServiceState::Ready("LSP and preview server are ready".to_owned());
                    if self.preview.tinymist_preview_enabled {
                        self.preview.interactive_url = Some(url);
                        self.preview.status = PreviewStatus::Ready(Duration::ZERO);
                        #[cfg(any(target_os = "macos", target_os = "windows"))]
                        let reusing_webview = {
                            let reusing = self.webview.is_some();
                            // A replacement server can receive the same local
                            // URL as its predecessor. URL equality therefore
                            // cannot prove that the retained WebView is showing
                            // the new pinned entry; force one navigation for
                            // every ready event when the surface is reused.
                            self.webview_reload_pending = reusing;
                            reusing
                        };
                        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                        let reusing_webview = false;
                        self.preview.webview_state = if reusing_webview {
                            ServiceState::Starting(
                                "Loading the pinned entry in the existing preview".to_owned(),
                            )
                        } else {
                            ServiceState::Starting("Embedding the vector preview".to_owned())
                        };
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
                        self.manual_format_revision = None;
                        self.notice = Some(Notice {
                            message: format!("Formatting failed: {message}"),
                            kind: NoticeKind::Error,
                        });
                        continue;
                    }
                    if stage == "preview" {
                        self.preview.interactive_url = None;
                        self.preview.webview_state = ServiceState::Failed(message.clone());
                    }
                    let detail = format!("{stage}: {message}");
                    self.preview.tinymist_state = if fatal {
                        self.preview.tinymist_lsp_ready = false;
                        self.preview.interactive_url = None;
                        self.preview.webview_state = ServiceState::Failed(
                            "Tinymist stopped before the embedded preview was available".to_owned(),
                        );
                        ServiceState::Failed(detail)
                    } else {
                        ServiceState::Degraded(detail)
                    };
                    if self.preview_processing_enabled() {
                        self.schedule_compile_now();
                    }
                }
                TinymistEvent::Stopped { reason, .. } => {
                    self.preview.tinymist_lsp_ready = false;
                    self.preview.interactive_url = None;
                    let detail = match &self.preview.tinymist_state {
                        ServiceState::Failed(previous) if previous != &reason => {
                            format!("{previous}; {reason}")
                        }
                        _ => reason,
                    };
                    self.preview.tinymist_state = ServiceState::Failed(detail);
                    self.preview.webview_state =
                        ServiceState::Failed("Tinymist preview is no longer running".to_owned());
                    self.schedule_compile_now();
                }
                _ => {}
            }
        }
    }

    fn receive_web_links(&mut self) {
        let targets = self.web_link_receiver.try_iter().collect::<Vec<_>>();
        for target in targets {
            self.handle_web_link(&target);
        }
    }

    fn handle_web_link(&mut self, target: &str) {
        self.follow_preview_link(target);
    }

    fn follow_preview_link(&mut self, target: &str) {
        if let Some(page) = internal_pdf_page_target(target) {
            self.preview.requested_page =
                Some(page.min(self.preview.pages.len().saturating_sub(1)));
            return;
        }

        if let Some(target) = normalize_browser_link_target(target) {
            if let Err(error) = open_in_system_browser(&target) {
                self.notice = Some(Notice {
                    message: error,
                    kind: NoticeKind::Error,
                });
            }
            return;
        }

        let url = match url::Url::parse(target) {
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
                let Ok(url) = base.join(target) else {
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

        if matches!(url.scheme(), "mailto" | "tel") {
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
            .document
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
        if self.document.kind == DocumentKind::Pdf {
            if self.preview.pages.is_empty() {
                self.pending_asset_page = page;
            } else if let Some(page) = page {
                self.preview.requested_page =
                    Some(page.min(self.preview.pages.len().saturating_sub(1)));
            }
        } else if self.document.kind.is_editable()
            && let Some((line, column)) = source_position
        {
            let char_index = char_index_at_line_column(&self.document.source, line, column);
            self.pending_editor_selection = Some(char_index..char_index);
            self.editor_attention = Some(EditorAttention {
                char_index,
                started: Instant::now(),
            });
            if self.document.kind.is_typst() {
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
        let virtual_untitled =
            self.document.path.is_none() && path == self.tinymist_document_path();
        let same_document = virtual_untitled
            || self
                .document
                .path
                .as_ref()
                .is_some_and(|current| same_path(current, &path));
        if !same_document {
            if path
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("typ"))
            {
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
            let range = range_to_char_range(&self.document.source, selection);
            self.editor_attention = Some(EditorAttention {
                char_index: range.start,
                started: Instant::now(),
            });
            self.pending_editor_selection = Some(range);
        }
        self.view_mode = ViewMode::Split;
    }

    fn jump_source_to_preview(&mut self, char_index: usize) {
        if !self.document.kind.is_typst() || !self.interactive_preview_active() {
            return;
        }
        let Some(generation) = self.tinymist_generation else {
            return;
        };
        let (line, character) = scalar_position_at_char(&self.document.source, char_index);
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
        preview_visible_for(
            self.document.kind,
            self.view_mode,
            self.designated_preview_path().is_some(),
        )
    }

    fn interactive_preview_requested(&self) -> bool {
        self.preview.interactive_requested(
            self.typst_preview_available(),
            cfg!(any(target_os = "macos", target_os = "windows")),
        )
    }

    fn tinymist_session_requested(&self) -> bool {
        self.preview.session_requested(
            self.typst_preview_available(),
            cfg!(any(target_os = "macos", target_os = "windows")),
        )
    }

    fn preview_processing_enabled(&self) -> bool {
        self.typst_preview_available()
            && (self.document_workflow.pending_export.is_some() || self.raster_preview_required())
    }

    fn raster_preview_required(&self) -> bool {
        self.preview.raster_required(
            self.interactive_preview_requested(),
            self.captures.has_pending_for("main"),
        )
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn fail_local_webview(&mut self, message: String) {
        let raster_was_required = self.raster_preview_required();
        self.webview = None;
        self.webview_url = None;
        self.webview_reload_pending = false;
        self.webview_navigation = None;
        self.preview.webview_state = ServiceState::Failed(message);
        let raster_is_required = self.raster_preview_required();
        if raster_fallback_compile_needed(
            self.typst_preview_available(),
            self.may_run_compilation(),
            raster_was_required,
            raster_is_required,
        ) {
            self.schedule_compile_now();
        }
    }

    fn sync_preview_visibility(&mut self) {
        let visible = self.preview_visible();
        let became_visible = !self.preview.was_visible && visible;
        self.preview.was_visible = visible;

        if !visible {
            self.hide_webview();
        }

        if self.preview.tinymist_preview_enabled != self.tinymist_session_requested() {
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
            && version.is_some_and(|version| version != revision_as_i32(self.document.revision))
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
        self.preview.tinymist_diagnostics = converted;
        self.mark_diagnostics_changed();
    }

    fn interactive_preview_active(&self) -> bool {
        self.preview.interactive_active(
            self.typst_preview_available(),
            cfg!(any(target_os = "macos", target_os = "windows")),
        )
    }

    fn should_attempt_interactive_preview(&self) -> bool {
        self.preview.should_attempt_interactive(
            self.typst_preview_available(),
            cfg!(any(target_os = "macos", target_os = "windows")),
        )
    }

    fn interactive_preview_transitioning(&self) -> bool {
        self.preview.interactive_transitioning(
            self.typst_preview_available(),
            cfg!(any(target_os = "macos", target_os = "windows")),
        )
    }

    fn preview_fallback_reason(&self) -> Option<String> {
        self.preview.fallback_reason(
            self.typst_preview_available(),
            self.preview_visible(),
            cfg!(any(target_os = "macos", target_os = "windows")),
        )
    }

    fn preview_backend_label(&self) -> &'static str {
        if !self.typst_preview_available() {
            return match self.document.kind {
                DocumentKind::Pdf => "Rasterised PDF",
                DocumentKind::Image => "Image",
                DocumentKind::Text => "Text editor",
                DocumentKind::Typst => unreachable!(),
            };
        }
        self.preview.backend_label(
            self.typst_preview_available(),
            cfg!(any(target_os = "macos", target_os = "windows")),
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
        let key = self
            .preview
            .raster_key
            .unwrap_or_else(|| ArtifactKey::unversioned(self.document.revision));
        let dark = self.preview.dark
            && (self.typst_preview_available() || self.document.kind != DocumentKind::Image);
        for (index, page) in self.preview.pages.iter_mut().enumerate() {
            let pixels = if dark {
                dark_preview_rgba(&page.rgba)
            } else {
                page.rgba.clone()
            };
            page.texture = context.load_texture(
                format!("preview-{}-{}-{index}-{dark}", key.revision, key.generation),
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

            theme::show_logo(ui);
            let title = format!(
                "{}{}",
                self.document_name(),
                if self.is_dirty() { "*" } else { "" }
            );
            // Reserve the dirty marker's slot even while the document is
            // clean, so saving never shifts the menu and view controls.
            let natural_title_width = (self.document_name().chars().count() + 1) as f32
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
                theme::nonselectable_label(RichText::new(&title).strong())
                    .truncate()
                    .sense(Sense::click()),
            );
            native_hover_text(
                title_response.clone(),
                format!(
                    "{}\nDouble-click to rename",
                    self.document.path.as_ref().map_or_else(
                        || "Unsaved document".to_owned(),
                        |path| path.display().to_string()
                    )
                ),
            );
            if title_response.double_clicked() && !self.document_flow_busy() {
                if let Some(path) = self.document.path.clone() {
                    self.begin_rename(path);
                } else {
                    self.save_as(frame);
                }
            }
            if self.settings.titlebar_menus {
                ui.separator();
                self.show_file_menu(ui);
                self.show_edit_menu(ui);
                self.show_view_menu(ui);
                ui.separator();
            }
            if native_hover_text(ui.button("Find"), "Find · Cmd+F").clicked() {
                self.toggle_find();
            }
            let (pause_label, pause_hint) = compilation_toggle_copy(self.compilation_paused);
            if native_hover_text(
                ui.add_enabled(
                    self.typst_preview_available(),
                    egui::Button::new(pause_label).selected(self.compilation_paused),
                ),
                pause_hint,
            )
            .clicked()
            {
                self.toggle_compilation_paused();
            }
            if native_hover_text(
                ui.add_enabled(self.typst_preview_available(), egui::Button::new("Compile")),
                "Compile PDF beside the Typst source · Cmd+R",
            )
            .clicked()
            {
                self.compile_pdf(frame);
            }

            // Consuming the remaining width with a right-to-left layout keeps
            // the view controls pinned to the opposite edge while the title is
            // the only item that contracts at narrow window sizes.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if native_hover_text(
                    ui.selectable_label(
                        self.problems_visible,
                        if compact { "!" } else { "Problems" },
                    ),
                    "Toggle compiler diagnostics · Cmd+5",
                )
                .clicked()
                {
                    self.problems_visible = !self.problems_visible;
                }
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
                        self.settings_visible,
                        if compact { "Set" } else { "Settings" },
                    ),
                    "Open Settings in a separate window",
                )
                .clicked()
                {
                    self.toggle_settings();
                }
            });
        });
    }

    fn show_file_menu(&mut self, ui: &mut egui::Ui) {
        let selected = matches!(self.app_popup, Some(AppPopup::File { .. }));
        let response = ui.selectable_label(selected, "File");
        if response.clicked() {
            if selected {
                self.close_app_popup();
            } else {
                self.open_app_popup(AppPopup::File {
                    anchor: response.rect.left_bottom(),
                });
            }
        }
    }

    fn show_edit_menu(&mut self, ui: &mut egui::Ui) {
        let selected = matches!(self.app_popup, Some(AppPopup::Edit { .. }));
        let response = ui.selectable_label(selected, "Edit");
        if response.clicked() {
            if selected {
                self.close_app_popup();
            } else {
                self.open_app_popup(AppPopup::Edit {
                    anchor: response.rect.left_bottom(),
                });
            }
        }
    }

    fn show_view_menu(&mut self, ui: &mut egui::Ui) {
        let selected = matches!(self.app_popup, Some(AppPopup::View { .. }));
        let response = ui.selectable_label(selected, "View");
        if response.clicked() {
            if selected {
                self.close_app_popup();
            } else {
                self.open_app_popup(AppPopup::View {
                    anchor: response.rect.left_bottom(),
                });
            }
        }
    }

    fn editor_history_availability(&self, _context: &egui::Context) -> (bool, bool) {
        self.document.history_availability()
    }

    fn undo_editor(&mut self, context: &egui::Context, redo: bool) {
        if !self.document.kind.is_editable() {
            return;
        }
        let current = self.editor_snapshot(context);
        let next = self.document.history_step(redo, current);
        let Some(next) = next else {
            return;
        };
        let changed = next.source.as_ref() != self.document.source;
        self.document.source = next.source.to_string();
        self.store_editor_cursor(context, next.cursor);
        self.document.reset_editor_history = false;
        let editor_id = source_editor_id(context);
        context.memory_mut(|memory| memory.request_focus(editor_id));
        if changed {
            self.search.clear();
            self.mark_edited();
        }
    }

    fn editor_snapshot(&mut self, context: &egui::Context) -> EditorSnapshot {
        self.prepare_editor_source_data();
        let metrics = self.editor_data.source_metrics();
        let cursor = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .and_then(|state| state.cursor.char_range())
            .unwrap_or_else(|| CCursorRange::one(CCursor::new(metrics.char_count)));
        EditorSnapshot {
            source: self.editor_data.source_snapshot(),
            cursor: clamp_cursor_range(cursor, metrics.char_count),
        }
    }

    fn store_editor_cursor(&self, context: &egui::Context, cursor: CCursorRange) {
        let mut state = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .unwrap_or_default();
        state.clear_undoer();
        state.cursor.set_char_range(Some(clamp_cursor_range(
            cursor,
            self.document.source.chars().count(),
        )));
        state.store(context, source_editor_id(context));
    }

    fn record_editor_undo_point(&mut self, context: &egui::Context) {
        let snapshot = self.editor_snapshot(context);
        self.push_editor_undo_snapshot(snapshot);
    }

    fn push_editor_undo_snapshot(&mut self, snapshot: EditorSnapshot) {
        self.document.push_undo_snapshot(snapshot);
    }

    fn selected_editor_chars(&self, context: &egui::Context) -> Option<Range<usize>> {
        let state = egui::text_edit::TextEditState::load(context, source_editor_id(context))?;
        let range = state.cursor.char_range()?.as_sorted_char_range();
        let len = self.document.source.chars().count();
        let range = range.start.0.min(len)..range.end.0.min(len);
        (range.start != range.end).then_some(range)
    }

    fn copy_editor_selection(&mut self, context: &egui::Context) {
        let Some(range) = self.selected_editor_chars(context) else {
            return;
        };
        self.prepare_editor_source_data();
        let bytes = self.editor_data.char_range_to_byte(range);
        context.copy_text(self.document.source[bytes].to_owned());
    }

    fn cut_editor_selection(&mut self, context: &egui::Context) {
        if !self.document.kind.is_editable() {
            return;
        }
        let Some(range) = self.selected_editor_chars(context) else {
            return;
        };
        self.record_editor_undo_point(context);
        let bytes = self.editor_data.char_range_to_byte(range.clone());
        context.copy_text(self.document.source[bytes.clone()].to_owned());
        self.document.source.replace_range(bytes, "");
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
        if self.document.kind.is_editable() {
            if self.document.kind.is_typst() && !self.view_mode.shows_code() {
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
                CCursor::new(self.document.source.chars().count()),
            )));
            state.store(context, editor_id);
            context.memory_mut(|memory| memory.request_focus(editor_id));
        }
    }

    fn toggle_comments(&mut self, context: &egui::Context) {
        if !self.document.kind.is_editable() {
            return;
        }
        let editor_id = source_editor_id(context);
        let state = egui::text_edit::TextEditState::load(context, editor_id);
        let cursor = state
            .as_ref()
            .and_then(|state| state.cursor.char_range())
            .map_or(0, |range| range.primary.index.0)
            .min(self.document.source.chars().count());
        let selection = state
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.as_sorted_char_range())
            .map(|range| {
                range.start.0.min(self.document.source.chars().count())
                    ..range.end.0.min(self.document.source.chars().count())
            })
            .filter(|range| range.start != range.end);
        let (source, mapped_range) = toggle_line_comments(
            &self.document.source,
            selection.clone().unwrap_or(cursor..cursor),
            "// ",
        );
        if source == self.document.source {
            return;
        }
        self.record_editor_undo_point(context);
        self.document.source = source;
        self.search.clear();
        self.mark_edited();
        let cursor = if selection.is_some() {
            CCursorRange::two(
                CCursor::new(mapped_range.start),
                CCursor::new(mapped_range.end),
            )
        } else {
            CCursorRange::one(CCursor::new(mapped_range.start))
        };
        self.store_editor_cursor(context, cursor);
        context.memory_mut(|memory| memory.request_focus(editor_id));
    }

    fn request_format_document(&mut self) {
        if !self.document.kind.is_typst() {
            self.manual_format_revision = None;
            return;
        }
        if !self.preview.tinymist_lsp_ready {
            self.manual_format_revision = None;
            self.notice = Some(Notice {
                message: "Tinymist is still starting; try formatting again in a moment".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        let (Some(generation), Some(uri)) = (self.tinymist_generation, self.tinymist_uri.clone())
        else {
            self.manual_format_revision = None;
            self.notice = Some(Notice {
                message: "Tinymist is still starting; try formatting again in a moment".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        };
        // Formatting is explicit work: send edits accumulated while automatic
        // compilation was paused before requesting this exact buffer version.
        if let Err(error) = self.sync_tinymist_change() {
            self.manual_format_revision = None;
            self.notice = Some(Notice {
                message: format!("Could not prepare the document for formatting: {error}"),
                kind: NoticeKind::Error,
            });
            return;
        }
        match self.tinymist.format_document(
            generation,
            uri,
            revision_as_i32(self.document.revision),
        ) {
            Ok(()) => {
                self.notice = Some(Notice {
                    message: "Formatting with Tinymist…".to_owned(),
                    kind: NoticeKind::Info,
                });
            }
            Err(error) => {
                self.manual_format_revision = None;
                self.notice = Some(Notice {
                    message: format!("Could not request formatting: {error}"),
                    kind: NoticeKind::Error,
                });
            }
        }
    }

    fn request_format_after_manual_save(&mut self) {
        if self.document.kind.is_typst() {
            self.manual_format_revision = Some(self.document.revision);
            self.request_format_document();
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
            || version != revision_as_i32(self.document.revision)
        {
            return;
        }
        let Some(edits) = edits else {
            self.manual_format_revision = None;
            self.notice = Some(Notice {
                message: "Tinymist did not provide a formatter for this document".to_owned(),
                kind: NoticeKind::Error,
            });
            return;
        };
        if edits.is_empty() {
            self.manual_format_revision = None;
            self.notice = Some(Notice {
                message: "Document is already formatted".to_owned(),
                kind: NoticeKind::Success,
            });
            return;
        }
        let cursor = if self.document.reset_editor_history {
            CCursorRange::one(CCursor::new(0))
        } else {
            self.editor_snapshot(context).cursor
        };
        let applied = match apply_text_edits(
            &self.document.source,
            &edits,
            [cursor.primary.index.0, cursor.secondary.index.0],
        ) {
            Ok(applied) => applied,
            Err(error) => {
                self.manual_format_revision = None;
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
        if formatted == self.document.source {
            self.manual_format_revision = None;
            self.notice = Some(Notice {
                message: "Document is already formatted".to_owned(),
                kind: NoticeKind::Success,
            });
            return;
        }
        self.pending_editor_selection = None;
        self.prepare_editor_source_data();
        let source = self.editor_data.source_snapshot();
        self.push_editor_undo_snapshot(EditorSnapshot { source, cursor });
        self.document.source = formatted;
        self.store_editor_cursor(context, mapped_cursor);
        self.document.reset_editor_history = false;
        self.search.clear();
        let save_after_format = self.manual_format_revision == Some(self.document.revision);
        self.manual_format_revision = None;
        self.mark_edited();
        let saved_after_format = if save_after_format {
            if let Some(path) = self.document.path.clone() {
                if !self.save_to_with_intent(path, SaveIntent::Explicit) {
                    return;
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        self.notice = Some(Notice {
            message: if saved_after_format {
                "Formatted and saved with Tinymist".to_owned()
            } else {
                "Formatted with Tinymist".to_owned()
            },
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
        if self.rename_dialog.is_none()
            || self.document_workflow.modal.is_some()
            || self.rename_overlay_suspended
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
        let Some(dialog) = &mut self.rename_dialog else {
            return;
        };
        let mut submit = false;
        let mut cancel = false;
        let mut overlay_had_focus = self.rename_overlay_had_focus;
        let mut suspend_overlay = false;
        let captures = self.captures.clone();
        let spec =
            ChildViewSpec::modal("tiptoptyp-rename-overlay", "Rename", window_rect, "rename");
        ChildViewHost::show(context, &captures, spec, theme, &style, |ui, input| {
            cancel |= {
                if input.focused == Some(true) {
                    overlay_had_focus = true;
                }
                suspend_overlay |= overlay_had_focus && input.focused == Some(false);
                input.close_requested || input.escape_pressed
            };
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
        });
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
        if !self.workspace_chooser_visible || self.document_workflow.modal.is_some() {
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
                                ui.label(
                                    RichText::new("Recent")
                                        .size(theme::TYPE.supporting)
                                        .strong(),
                                );
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
        if self.document_workflow.modal_suspended {
            return;
        }
        let Some(modal) = self.document_workflow.modal.clone() else {
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
        let mut overlay_had_focus = self.document_workflow.modal_had_focus;
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
        self.document_workflow.modal_had_focus = overlay_had_focus;
        if suspend_overlay {
            self.document_workflow.modal_suspended = true;
        }

        let Some(choice) = choice else {
            return;
        };
        self.document_workflow.clear_modal();
        context.send_viewport_cmd(egui::ViewportCommand::Focus);
        match (modal, choice) {
            (AppModal::Alert { .. }, _) => {}
            (AppModal::Unsaved { pending, .. }, AppModalChoice::Primary) => {
                self.document_workflow.pending_action = Some(PendingDocumentAction {
                    action: DeferredDocumentAction::SaveThen(Box::new(pending)),
                    key: self.document.key(),
                    allow_discard: true,
                    description: "saving the current document".to_owned(),
                });
            }
            (AppModal::Unsaved { mut pending, .. }, AppModalChoice::Secondary) => {
                self.document_workflow.post_save_action = None;
                pending.key = self.document.key();
                pending.allow_discard = true;
                self.document_workflow.pending_action = Some(pending);
            }
            (AppModal::Unsaved { .. }, AppModalChoice::Cancel) => {
                self.document_workflow.post_save_action = None;
                self.schedule_autosave_if_needed();
            }
            (
                AppModal::Overwrite {
                    path,
                    key,
                    expected_disk_fingerprint,
                    observed_disk_fingerprint,
                    ..
                },
                AppModalChoice::Primary,
            ) => {
                self.document_workflow.pending_action = Some(PendingDocumentAction {
                    action: DeferredDocumentAction::ForceSave {
                        path,
                        key,
                        expected_disk_fingerprint,
                        observed_disk_fingerprint,
                    },
                    key: self.document.key(),
                    allow_discard: true,
                    description: "overwriting the current file".to_owned(),
                });
            }
            (AppModal::Overwrite { .. }, _) => {
                self.document_workflow.post_save_action = None;
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
            .document
            .path
            .as_ref()
            .is_some_and(|current| same_path(current, &old_path));
        let renames_preview_document = self
            .designated_preview_path()
            .is_some_and(|preview| same_path(&preview, &old_path));
        let preserve_designated_preview = renames_current_document
            && !renames_preview_document
            && self.should_keep_designated_preview(&new_path);
        let reload_binary_document = renames_current_document && self.document.kind.preview_only();
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
            // bytes. Using `self.document.source` here would pass an empty buffer for
            // PDFs/images and could incorrectly turn them into editable text.
            self.load_path(new_path.clone());
        } else if renames_current_document {
            self.document.path = Some(new_path.clone());
            self.document.epoch = self.document.epoch.wrapping_add(1);
            self.document.kind = DocumentKind::detect(&new_path, self.document.source.as_bytes())
                .ok()
                .filter(|kind| kind.is_editable())
                .unwrap_or(DocumentKind::Text);
            self.remember_open_document(&new_path);
            if preserve_designated_preview {
                self.reopen_tinymist_current_document(&new_path, self.document.kind);
                self.refresh_workspace();
            } else {
                self.reset_document_services();
            }
            if self.document.kind.is_typst() {
                self.schedule_compile_now();
            } else {
                self.preview.status = PreviewStatus::Ready(Duration::ZERO);
            }
        }
        self.refresh_workspace();
        if renames_preview_document && !renames_current_document {
            // Renaming the current document already reset its services above.
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
        if !self.settings_visible
            || self.document_workflow.modal.is_some()
            || self.rename_dialog.is_some()
        {
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
        let mut close_requested = false;
        let captures = self.captures.clone();
        let web_link_sender = self.web_link_sender.clone();
        let spec = ChildViewSpec::persistent(
            "tiptoptyp-settings",
            "tiptoptyp Settings",
            [
                METRICS.chrome.settings_width,
                METRICS.chrome.settings_height,
            ],
            METRICS.chrome.settings_min_size,
            "settings",
        );
        ChildViewHost::show(
            context,
            &captures,
            spec,
            active_theme,
            &style,
            |ui, input| {
                let settings_tooltip_id = settings_hover_tooltip_id(ui.ctx());
                ui.ctx()
                    .data_mut(|data| data.remove::<HoverTooltipOverlay>(settings_tooltip_id));
                close_requested |= input.close_requested;
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
                            ui.add_space(
                                METRICS.toolbar.traffic_lights_fallback_width
                                    + METRICS.toolbar.traffic_lights_gap,
                            );
                            theme::show_logo(ui);
                            ui.label(RichText::new("Settings").strong());
                            #[cfg(not(target_os = "macos"))]
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
                        &web_link_sender,
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
                        &web_link_sender,
                    );
                }
            },
        );
        if close_requested {
            self.settings_visible = false;
            self.staged_ui_font_weight = None;
            self.staged_code_font_weight = None;
            // macOS does not consistently reactivate the parent after its
            // child settings window closes. Explicit focus makes the next
            // toolbar/menu click actionable instead of activation-only.
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn show_typst_overrides_window(&mut self, context: &egui::Context) {
        if !self.typst_overrides_visible
            || self.document_workflow.modal.is_some()
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
            overrides
                .get_mut_or_default(TypstSyntaxRole::Function)
                .weight = Some(theme::FONT_WEIGHT_BOLD);
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
        let web_link_sender = self.web_link_sender.clone();
        let mut close_requested = false;

        let viewport = child_viewport_builder(
            egui::ViewportBuilder::default()
                .with_title("tiptoptyp Typst Overrides")
                .with_inner_size([
                    METRICS.chrome.typst_overrides_width,
                    METRICS.chrome.typst_overrides_height,
                ])
                .with_min_inner_size(METRICS.chrome.typst_overrides_min_size)
                .with_fullsize_content_view(true)
                .with_title_shown(false)
                .with_titlebar_shown(false)
                .with_maximize_button(false)
                .with_maximized(false)
                .with_fullscreen(false),
        );
        context.show_viewport_immediate(
            scoped_child_viewport_id(context, "tiptoptyp-typst-overrides"),
            viewport,
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
                            #[cfg(target_os = "macos")]
                            ui.add_space(
                                METRICS.toolbar.traffic_lights_fallback_width
                                    + METRICS.toolbar.traffic_lights_gap,
                            );
                            theme::show_logo(ui);
                            ui.label(RichText::new("Typst overrides").strong());
                            #[cfg(not(target_os = "macos"))]
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
                                    .size(theme::TYPE.supporting)
                                    .weak(),
                            );
                        });
                        if let Some(reason) = &fallback {
                            fallback_notice(ui, "Theme fallback", reason);
                        }
                        ui.add_space(theme::SPACE.small);
                        let scroll_area = egui::ScrollArea::both()
                            .id_salt("typst-overrides-scroll")
                            .auto_shrink([false, false]);
                        let scroll_area = if deterministic {
                            // Snapshot scenes must not inherit the persisted scroll memory of
                            // an earlier interactive window. Otherwise a fresh capture can begin
                            // halfway through the table and hide the headers being verified.
                            scroll_area.scroll_offset(Vec2::ZERO)
                        } else {
                            scroll_area
                        };
                        scroll_area.show(ui, |ui| {
                            show_typst_override_editor(
                                ui,
                                edited.typst_overrides.for_dark_mut(rendered_dark),
                                syntax_palette,
                                &preview_theme.syntect_theme,
                                &self.font_configuration.editor_weight_support,
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
                        &web_link_sender,
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

    fn show_diagnostic_tooltip_window(&mut self, context: &egui::Context) {
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
                "```typc\ntext(body, size: length = 1em, fill: color = black)\n```\nDisplays content as text with the selected size and fill."
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
        let interaction_id = tooltip_interaction_id(context);
        let geometry_id = tooltip_geometry_id(context);
        let identity = tooltip_identity(origin, &detail);
        let interaction =
            context.data(|data| data.get_temp::<TooltipInteractionState>(interaction_id));
        if interaction.is_some_and(|state| state.identity == identity && state.dismissed) {
            self.diagnostic_tooltip = None;
            context.data_mut(|data| {
                data.remove::<HoverTooltipOverlay>(native_tooltip_id);
                data.remove::<TooltipGeometry>(geometry_id);
                data.remove::<TooltipInteractionState>(interaction_id);
            });
            return;
        }
        let deterministic_scene = matches!(
            self.snapshot_scene,
            Some(UiSnapshotScene::DiagnosticTooltip | UiSnapshotScene::FunctionTooltip)
        );
        let root_focused = context.input(|input| {
            input.viewport().focused == Some(true) && input.viewport().visible() != Some(false)
        });
        let geometry = context.data(|data| data.get_temp::<TooltipGeometry>(geometry_id));
        let handoff_active = native_tooltip_handoff_active(context, false);
        let root_ready = tooltip_viewport_should_render(
            deterministic_scene,
            root_focused,
            handoff_active,
            identity,
            geometry,
            interaction,
        );
        if !root_ready
            || self.settings_visible
            || self.rename_dialog.is_some()
            || self.app_popup.is_some()
            || self.document_workflow.modal.is_some()
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
            &self.web_link_sender,
        );
    }

    fn show_app_popup_window(&mut self, context: &egui::Context) {
        if self.document_workflow.modal.is_some()
            || self.settings_visible
            || self.typst_overrides_visible
            || self.workspace_chooser_visible
            || self.rename_dialog.is_some()
        {
            self.close_app_popup();
            return;
        }
        let Some(popup) = self.app_popup.clone() else {
            return;
        };
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            self.close_app_popup();
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let (anchor, desired_size) = match &popup {
            AppPopup::File { anchor } => (*anchor, METRICS.menu.file_size),
            AppPopup::Edit { anchor } => (*anchor, METRICS.menu.edit_size),
            AppPopup::View { anchor } => (*anchor, METRICS.menu.view_size),
            AppPopup::Workspace {
                anchor, is_file, ..
            } => (*anchor, workspace_context_menu_size(*is_file)),
            AppPopup::Editor { anchor, link } => {
                (*anchor, editor_context_menu_size(link.is_some()))
            }
            AppPopup::StatusLog { anchor } => {
                (*anchor, status_log_popup_size(self.status_log.len()))
            }
            AppPopup::FontSelector { anchor, .. } => (*anchor, METRICS.menu.font_selector_size),
        };
        let estimated_size = egui::vec2(
            desired_size
                .x
                .min((window_rect.width() - METRICS.popup.viewport_edge).max(1.0)),
            desired_size
                .y
                .min((window_rect.height() - METRICS.popup.viewport_edge).max(1.0)),
        );
        let menu_frame = theme::menu_card_frame(&style);
        let frame_margin = menu_frame.total_margin().sum();
        let menu_content_size = frame_content_size(estimated_size, frame_margin);
        let menu_width = menu_content_size.x;
        let menu_height = menu_content_size.y;
        let anchor = if matches!(popup, AppPopup::StatusLog { .. }) {
            clamp_popup_above_anchor(anchor, estimated_size, window_rect.size())
        } else {
            clamp_popup_anchor(anchor, estimated_size, window_rect.size())
        };
        let (can_undo, can_redo) = self.editor_history_availability(context);
        let has_selection = self.selected_editor_chars(context).is_some();
        let can_format = self.document.kind.is_typst();
        let can_sync_preview = self.document.kind.is_typst() && self.interactive_preview_active();
        let mut close = false;
        let mut action = None;
        let mut had_focus = self.app_popup_had_focus;
        let mut blur_started = self.app_popup_blur_started;
        let popup_generation = self.app_popup_generation;
        let captures = self.captures.clone();
        let spec = ChildViewSpec::dismiss_on_blur(
            "tiptoptyp-popup-overlay",
            "tiptoptyp menu",
            window_rect.min + anchor.to_vec2(),
            estimated_size,
            "popup",
        );

        ChildViewHost::show(context, &captures, spec, theme, &style, |ui, input| {
            close |= input.close_requested || input.escape_pressed;
            let now = Instant::now();
            close |=
                popup_focus_should_close(&mut had_focus, &mut blur_started, input.focused, now);
            if let Some(started) = blur_started {
                let elapsed = now.saturating_duration_since(started);
                if elapsed < POPUP_BLUR_GRACE {
                    ui.ctx().request_repaint_after(POPUP_BLUR_GRACE - elapsed);
                }
            }

            egui::Area::new(viewport_scoped_id(ui.ctx(), "app-popup-card"))
                .order(egui::Order::Foreground)
                .fixed_pos(Pos2::ZERO)
                .show(ui.ctx(), |ui| {
                    menu_frame.show(ui, |ui| {
                        ui.set_min_width(menu_width);
                        ui.set_max_width(menu_width);
                        egui::ScrollArea::vertical()
                            .id_salt(app_popup_scroll_id(popup_generation))
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
                                AppPopup::View { .. } => {
                                    show_view_popup_ui(ui, &mut action);
                                }
                                AppPopup::Workspace { path, is_file, .. } => {
                                    let preview_selected = self
                                        .designated_preview_path()
                                        .is_some_and(|preview| same_path(&preview, path));
                                    show_workspace_popup_ui(
                                        ui,
                                        path,
                                        *is_file,
                                        preview_selected,
                                        &mut action,
                                    );
                                }
                                AppPopup::Editor { link, .. } => {
                                    let mut editor_action = None;
                                    show_editor_context_menu_ui(
                                        ui,
                                        EditorContextMenuOptions {
                                            can_undo,
                                            can_redo,
                                            has_selection,
                                            can_format,
                                            can_sync_preview,
                                            link: link.as_deref(),
                                        },
                                        &mut editor_action,
                                    );
                                    if let Some(editor_action) = editor_action {
                                        action = Some(AppPopupAction::Editor(editor_action));
                                    }
                                }
                                AppPopup::StatusLog { .. } => {
                                    show_status_log_popup_ui(ui, &self.status_log);
                                }
                                AppPopup::FontSelector { target, .. } => {
                                    show_document_font_selector_ui(
                                        ui,
                                        &self.font_catalog,
                                        target,
                                        &mut action,
                                    );
                                }
                            });
                    });
                });
        });
        self.app_popup_had_focus = had_focus;
        self.app_popup_blur_started = blur_started;

        let action_selected = action.is_some();
        if close || action_selected {
            self.close_app_popup();
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
            AppPopupAction::Command(command) => self.execute_app_command(command, context, frame),
            AppPopupAction::Editor(EditorMenuAction::Command(command)) => {
                self.execute_app_command(command, context, frame)
            }
            AppPopupAction::Editor(EditorMenuAction::OpenLink(target)) => {
                self.follow_preview_link(&target)
            }
            AppPopupAction::Workspace(action) => match action {
                WorkspaceMenuAction::Open(path) => {
                    if self
                        .document
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
                WorkspaceMenuAction::OpenInNewWindow(path) => {
                    self.pending_window_requests
                        .push_back(EditorWindowRequest::Open(path));
                }
                WorkspaceMenuAction::TogglePreview(path) => {
                    self.toggle_file_for_preview(path, context)
                }
                WorkspaceMenuAction::Rename(path) => self.begin_rename(path),
                WorkspaceMenuAction::Copy { path, kind } => {
                    if let Some(text) = workspace_copy_text(&path, &self.project_root(), kind) {
                        context.copy_text(text);
                    } else {
                        self.notice = Some(Notice {
                            message: format!(
                                "Could not copy the {} for {}",
                                workspace_copy_kind_description(kind),
                                path.display()
                            ),
                            kind: NoticeKind::Error,
                        });
                    }
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
            AppPopupAction::SetDocumentFont { target, family } => {
                self.replace_document_font(target, &family, context);
            }
        }
    }

    fn replace_document_font(
        &mut self,
        target: FontArgumentTarget,
        family: &str,
        context: &egui::Context,
    ) {
        if target.value_range.end > self.document.source.len()
            || !self
                .document
                .source
                .is_char_boundary(target.value_range.start)
            || !self
                .document
                .source
                .is_char_boundary(target.value_range.end)
        {
            self.notice = Some(Notice {
                message: "The font argument changed before it could be updated".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        let snapshot = self.editor_snapshot(context);
        let replacement = family.replace('\\', "\\\\").replace('"', "\\\"");
        let selection_end = self.document.source[..target.value_range.start]
            .chars()
            .count()
            + replacement.chars().count();
        self.document
            .source
            .replace_range(target.value_range, &replacement);
        self.push_editor_undo_snapshot(snapshot);
        self.pending_editor_selection = Some(selection_end..selection_end);
        self.search.clear();
        self.mark_edited();
        self.notice = Some(Notice {
            message: format!("Set document font to {family}"),
            kind: NoticeKind::Success,
        });
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
        let settings_scroll = egui::ScrollArea::vertical()
            .id_salt("settings-scroll")
            .auto_shrink([false, false]);
        let settings_scroll = if deterministic_settings {
            // Deterministic scenes describe the complete Settings contract from
            // its first row, independently of persisted egui scroll memory.
            settings_scroll.vertical_scroll_offset(0.0)
        } else {
            settings_scroll
        };
        settings_scroll.show(ui, |ui| {
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
                        ui.label(
                            RichText::new("QA override")
                                .size(theme::TYPE.supporting)
                                .weak(),
                        );
                    }
                    ui.separator();
                    settings_inline_value(ui, "Active", theme_label(Some(effective_theme)));
                });
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new("Typst syntax").strong());
                    if ui.button("Overrides…").clicked() {
                        self.typst_overrides_dark = effective_theme == egui::Theme::Dark;
                        self.typst_overrides_visible = true;
                    }
                    ui.label(
                        RichText::new("Colours and decorations inherit from the selected theme")
                            .weak(),
                    );
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
                                let id = ui.make_persistent_id(egui::IdSalt::new((
                                    picker_id,
                                    self.snapshot_scene,
                                )));
                                egui::Popup::open_id(ui.ctx(), id.with("popup"));
                            }
                            let choice = edited.color_theme_mut(appearance);
                            let picker = egui::ComboBox::from_id_salt((
                                picker_id,
                                self.snapshot_scene,
                            ))
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
                            .size(theme::TYPE.supporting)
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
                        "Theme fallback",
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
                    ui.label(RichText::new("Interface scale").strong());
                    ui.add(
                        egui::Slider::new(&mut edited.ui_scale_percent, 75..=150)
                            .suffix("%")
                            .clamping(egui::SliderClamping::Always),
                    );
                    ui.separator();
                    ui.checkbox(&mut edited.titlebar_menus, "Show title-bar menus");
                });
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new("UI font").strong());
                    if let Some(selection) = show_font_family_picker(
                        ui,
                        "ui-font-family",
                        &self.font_catalog,
                        edited.ui_font_path.as_deref(),
                        edited.ui_font_family.as_deref(),
                        if edited.ui_font_monospace {
                            "Editor font"
                        } else {
                            "System UI"
                        },
                        true,
                    ) {
                        match selection {
                            FontPickerSelection::Default => {
                                edited.ui_font_path = None;
                                edited.ui_font_family = None;
                                edited.ui_font_face_index = 0;
                                edited.ui_font_monospace = false;
                            }
                            FontPickerSelection::Editor => {
                                edited.ui_font_path = None;
                                edited.ui_font_family = None;
                                edited.ui_font_face_index = 0;
                                edited.ui_font_monospace = true;
                            }
                            FontPickerSelection::Family {
                                name,
                                path,
                                face_index,
                            } => {
                                edited.ui_font_path = Some(path);
                                edited.ui_font_family = Some(name);
                                edited.ui_font_face_index = face_index;
                                edited.ui_font_monospace = false;
                            }
                        }
                    }
                    if ui.button("Choose…").clicked() {
                        self.choose_tool_binary(ToolPickerTarget::UiFont, frame, ui.ctx());
                    }
                    ui.separator();
                    show_font_weight_control(
                        ui,
                        "ui-font-weight",
                        &mut edited.ui_font_weight,
                        &mut self.staged_ui_font_weight,
                        self.font_configuration.ui_weight_support.as_ref(),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new("Code font").strong());
                    if let Some(selection) = show_font_family_picker(
                        ui,
                        "code-font-family",
                        &self.font_catalog,
                        edited.code_font_path.as_deref(),
                        edited.code_font_family.as_deref(),
                        "System monospace",
                        false,
                    ) {
                        match selection {
                            FontPickerSelection::Default | FontPickerSelection::Editor => {
                                edited.code_font_path = None;
                                edited.code_font_family = None;
                                edited.code_font_face_index = 0;
                            }
                            FontPickerSelection::Family {
                                name,
                                path,
                                face_index,
                            } => {
                                edited.code_font_path = Some(path);
                                edited.code_font_family = Some(name);
                                edited.code_font_face_index = face_index;
                            }
                        }
                    }
                    if ui.button("Choose…").clicked() {
                        self.choose_tool_binary(ToolPickerTarget::CodeFont, frame, ui.ctx());
                    }
                    ui.separator();
                    show_font_weight_control(
                        ui,
                        "code-font-weight",
                        &mut edited.code_font_weight,
                        &mut self.staged_code_font_weight,
                        self.font_configuration.code_weight_support.as_ref(),
                    );
                    if self.font_catalog_scan.is_running() {
                        ui.label(RichText::new("Scanning fonts…").weak());
                    } else if !self.font_catalog.workspace_directories().is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "{} workspace font folder{}",
                                self.font_catalog.workspace_directories().len(),
                                if self.font_catalog.workspace_directories().len() == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            ))
                            .weak(),
                        );
                    }
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
                    .size(theme::TYPE.supporting)
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
                    fallback_notice(ui, "Binary fallback active", reason);
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
                    fallback_notice(ui, "Binary fallback active", reason);
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
                    fallback_notice(ui, "Preview fallback active", &reason);
                }
                if !deterministic_settings
                    && self.preview.requested_backend == PreviewPreference::Interactive
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
                        show_service_status_chip(ui, "LSP", &self.preview.tinymist_state);
                        show_service_status_chip(ui, "Vector", &self.preview.webview_state);
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
                        ui.label(
                            RichText::new("Cmd+Shift+F12")
                                .size(theme::TYPE.supporting)
                                .weak(),
                        ),
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
        if !self.typst_preview_available() {
            return ServiceState::Disabled("The selected file is not compiled as Typst".to_owned());
        }
        if !self.may_run_compilation() {
            return ServiceState::Disabled("Automatic preview updates are paused".to_owned());
        }
        match self.preview.status {
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
                    .preview
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
        if self.document.kind == DocumentKind::Text && !self.typst_preview_available() {
            return ServiceState::Disabled("Text files do not need a preview renderer".to_owned());
        }
        if self.document.kind == DocumentKind::Image
            && !self.typst_preview_available()
            && !self.preview.pages.is_empty()
        {
            return ServiceState::Ready("The selected image decoded successfully".to_owned());
        }
        if self.raster_content_freshness() == Some(RasterContentFreshness::Current) {
            return ServiceState::Ready(format!(
                "Poppler rendered {} page(s) at {} DPI",
                self.preview.pages.len(),
                crate::compiler::PREVIEW_DPI
            ));
        }
        if self.raster_content_freshness() == Some(RasterContentFreshness::Stale) {
            return ServiceState::Degraded(format!(
                "Showing {} page(s) from the last successful build",
                self.preview.pages.len()
            ));
        }
        if let Some(error) = &self.preview.raster_error {
            return ServiceState::Degraded(error.clone());
        }
        match self.preview.status {
            PreviewStatus::Error => {
                ServiceState::Failed("No rasterized pages are available".to_owned())
            }
            PreviewStatus::Waiting | PreviewStatus::Compiling | PreviewStatus::Ready(_) => {
                ServiceState::Starting("Waiting for the watched PDF to render".to_owned())
            }
        }
    }

    fn raster_content_freshness(&self) -> Option<RasterContentFreshness> {
        self.preview.raster_freshness(self.document.revision)
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
            self.document.path.as_ref().and_then(|path| {
                path.strip_prefix(&snapshot.root)
                    .ok()
                    .and_then(|relative| snapshot.find(relative))
                    .map(|node| node.path.clone())
            })
        });
        let preview = snapshot
            .as_ref()
            .and_then(|snapshot| {
                preview_path.as_ref().and_then(|path| {
                    path.strip_prefix(&snapshot.root)
                        .ok()
                        .and_then(|relative| snapshot.find(relative))
                        .map(|node| node.path.clone())
                })
            })
            .or(preview_path);
        let mut open_path = None;
        let mut index_target = None;
        let mut popup_request = None;
        let dark_mode = ui.visuals().dark_mode;
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
                                    preview.as_deref(),
                                    dark_mode,
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
                .document
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
                .document
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
            self.open_app_popup(popup);
        }
    }

    fn show_find_bar(&mut self, ui: &mut egui::Ui) {
        let mut find_next = false;
        let mut find_previous = false;
        let mut replace_one = false;
        let mut replace_all = false;
        let search_revision = SearchRevision::new(self.document.epoch, self.document.revision);
        let search_results = self.search.results(
            &self.document.source,
            search_revision,
            &self.find_query,
            self.find_case_sensitive,
            self.find_regex,
        );
        let match_status = search_results.error().map_or_else(
            || format!("{} matches", search_results.len()),
            |_| "Invalid pattern".to_owned(),
        );

        ui.horizontal_wrapped(|ui| {
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
            if response.lost_focus()
                && let Some(step) = ui.input(|input| {
                    find_step_for_enter(input.key_pressed(egui::Key::Enter), input.modifiers.shift)
                })
            {
                match step {
                    FindStep::Next => find_next = true,
                    FindStep::Previous => find_previous = true,
                }
                // Single-line TextEdit relinquishes focus on Enter. Return it
                // immediately so repeated Enter/Shift+Enter keeps navigating.
                response.request_focus();
            }
            ui.label(
                RichText::new(match_status)
                    .size(theme::TYPE.supporting)
                    .weak(),
            );
            let case_label = if self.find_case_sensitive { "Aa" } else { "aa" };
            if native_hover_text(
                ui.selectable_label(self.find_case_sensitive, case_label),
                if self.find_case_sensitive {
                    "Case-sensitive matching"
                } else {
                    "Case-insensitive matching"
                },
            )
            .clicked()
            {
                self.find_case_sensitive = !self.find_case_sensitive;
                self.search.clear();
            }
            let regex_button = native_hover_text(
                ui.selectable_label(self.find_regex, ".*"),
                "Regular expression mode. Supports ., *, +, ?, [], ^, $, \\d, \\w, and \\s.",
            );
            regex_button.context_menu(|ui| {
                ui.label(RichText::new("Regular expressions").strong());
                ui.label(".  any character");
                ui.label("*  zero or more · +  one or more");
                ui.label("?  optional · ^  start · $  end");
                ui.label("[abc] [a-z] [^0-9]  character classes");
                ui.label("\\d digit · \\w word · \\s whitespace");
                ui.label("Replacement text is literal; capture expansion is not supported.");
            });
            if regex_button.clicked() {
                self.find_regex = !self.find_regex;
                self.search.clear();
            }
            if ui
                .selectable_label(self.replace_visible, "Replace")
                .on_hover_text("Show or hide replace fields")
                .clicked()
            {
                self.replace_visible = !self.replace_visible;
            }
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
                .previous(
                    &self.document.source,
                    search_revision,
                    &self.find_query,
                    self.find_case_sensitive,
                    self.find_regex,
                )
                .map(|matched| matched.char_range.clone());
        }
        if find_next {
            self.pending_editor_selection = self
                .search
                .next(
                    &self.document.source,
                    search_revision,
                    &self.find_query,
                    self.find_case_sensitive,
                    self.find_regex,
                )
                .map(|matched| matched.char_range.clone());
        }
        if replace_one {
            let before = self.document.source.clone();
            let snapshot = self.editor_snapshot(ui.ctx());
            let replaced = self.search.replace_one(
                &mut self.document.source,
                search_revision,
                &self.find_query,
                &self.replacement,
                self.find_case_sensitive,
                self.find_regex,
            );
            if replaced {
                self.pending_editor_selection = self
                    .search
                    .selected()
                    .map(|matched| matched.char_range.clone());
                if self.document.source != before {
                    self.push_editor_undo_snapshot(snapshot);
                    self.mark_edited();
                }
            }
        }
        if replace_all {
            let before = self.document.source.clone();
            let snapshot = self.editor_snapshot(ui.ctx());
            let count = self.search.replace_all(
                &mut self.document.source,
                search_revision,
                &self.find_query,
                &self.replacement,
                self.find_case_sensitive,
                self.find_regex,
            );
            if count > 0 && self.document.source != before {
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
        if self.document.reset_editor_history {
            let mut state =
                egui::text_edit::TextEditState::load(ui.ctx(), source_editor_id(ui.ctx()))
                    .unwrap_or_default();
            state.clear_undoer();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            state.store(ui.ctx(), source_editor_id(ui.ctx()));
            self.document.reset_editor_history = false;
        }
        let source_revision = self.editor_revision();
        self.prepare_editor_data();
        let source_metrics = self.editor_data.source_metrics();
        let line_diagnostics = self.editor_data.line_diagnostics();
        let available_width = ui.available_width();
        let line_count = source_metrics.line_count;
        let longest_line = source_metrics.longest_line_chars;
        let longest_diagnostic = self.editor_data.longest_diagnostic_chars();
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
        let document_kind = self.document.kind;
        let highlight_path = self.document.path.clone();
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
        let mut clicked_web_link = None;
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
                let editor = egui::TextEdit::multiline(&mut self.document.source)
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
                if changed {
                    self.editor_data
                        .prepare_source(source_revision.after_edit(), &self.document.source);
                }
                let mut current_char = output
                    .state
                    .cursor
                    .char_range()
                    .map(|range| range.primary.index.0);
                let line_rows = logical_line_row_ranges(&output.galley.rows);

                if let Some(range) = pending_selection {
                    let len = self.document.source.chars().count();
                    let range = range.start.min(len)..range.end.min(len);
                    let cursor_range =
                        CCursorRange::two(CCursor::new(range.start), CCursor::new(range.end));
                    output.state.cursor.set_char_range(Some(cursor_range));
                    current_char = Some(range.end);
                    output.state.clone().store(ui.ctx(), output.response.id);
                    if !self.find_visible {
                        output.response.request_focus();
                    }
                    let cursor_rect = output
                        .galley
                        .pos_from_cursor(CCursor::new(range.start))
                        .translate(output.galley_pos.to_vec2());
                    ui.scroll_to_rect(cursor_rect, Some(Align::Center));
                }

                paint_editor_line_backgrounds(
                    ui,
                    &output,
                    &self.document.source,
                    current_char,
                    attention,
                    &line_rows,
                    current_line_slot,
                );
                if let Some(tooltip) = paint_line_diagnostics(
                    ui,
                    &output,
                    &line_diagnostics,
                    &line_rows,
                    background_slots,
                ) && !native_tooltip_handoff_blocks(ui.ctx(), tooltip.origin)
                {
                    // Keep the last diagnostic payload while the pointer is
                    // crossing the root-local bridge or sitting in the
                    // native child. The root is deliberately no longer
                    // hovering the diagnostic row in that frame, but the
                    // popup still needs the payload in order to repaint and
                    // accept scroll input.
                    self.diagnostic_tooltip = Some(tooltip);
                }
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
                    if let Some(range) = typst_hover_token_range(&self.document.source, char_index)
                    {
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

                if document_kind.is_typst()
                    && output.response.hovered()
                    && ui.input(|input| input.modifiers.command)
                    && let Some(pointer) = ui.ctx().pointer_hover_pos()
                {
                    let char_index = output
                        .galley
                        .cursor_from_pos(pointer - output.galley_pos)
                        .index
                        .0;
                    if editor_web_link_at(&mut self.editor_data, char_index).is_some() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }

                if output.response.secondary_clicked() {
                    let anchor = ui
                        .ctx()
                        .pointer_latest_pos()
                        .unwrap_or_else(|| output.response.rect.center());
                    let char_index = output.response.interact_pointer_pos().map(|pointer| {
                        output
                            .galley
                            .cursor_from_pos(pointer - output.galley_pos)
                            .index
                            .0
                    });
                    let target = char_index
                        .and_then(|char_index| self.editor_data.font_argument_at(char_index));
                    let link = char_index.and_then(|char_index| {
                        editor_web_link_at(&mut self.editor_data, char_index)
                    });
                    popup_request = Some(match target {
                        Some(target) => AppPopup::FontSelector { anchor, target },
                        None => AppPopup::Editor { anchor, link },
                    });
                }

                let clicked_link = if document_kind.is_typst() {
                    output.response.interact_pointer_pos().and_then(|pointer| {
                        let char_index = output
                            .galley
                            .cursor_from_pos(pointer - output.galley_pos)
                            .index
                            .0;
                        editor_web_link_click_target(
                            &mut self.editor_data,
                            char_index,
                            output.response.clicked(),
                            ui.input(|input| input.modifiers.command),
                        )
                    })
                } else {
                    None
                };
                clicked_web_link = clicked_link.clone();

                let jump_gesture = source_preview_jump_gesture(
                    source_preview_trigger,
                    output.response.clicked(),
                    output.response.double_clicked(),
                    ui.input(|input| input.modifiers.command),
                    clicked_link.is_some(),
                );
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
                        self.document.source.chars().count(),
                    ))));
                state.store(ui.ctx(), editor_id);
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(editor_id));
            }
        }

        if let Some(popup) = popup_request {
            self.open_app_popup(popup);
        }

        if changed {
            self.push_editor_undo_snapshot(snapshot_before_edit);
            self.search.clear();
            self.mark_edited();
        }
        if let Some(char_index) = preview_jump_char {
            self.jump_source_to_preview(char_index);
        }
        if let Some(target) = clicked_web_link {
            self.follow_preview_link(&target);
        }

        if self.find_visible {
            let context = ui.ctx().clone();
            let overlay_width = (scroll_output.inner_rect.width() - 4.0 * theme::SPACE.content)
                .clamp(1.0, METRICS.editor.find_overlay_max_width);
            let anchor = scroll_output.inner_rect.left_top()
                + egui::vec2(theme::SPACE.content, theme::SPACE.content);
            egui::Area::new(viewport_scoped_id(&context, "find-replace-overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(anchor)
                .constrain_to(ui.clip_rect())
                .show(&context, |ui| {
                    theme::popup_card_frame(ui.style()).show(ui, |ui| {
                        ui.set_max_width(overlay_width);
                        self.show_find_bar(ui);
                    });
                });
        }
    }

    fn update_editor_hover(&mut self, ui: &mut egui::Ui, hovered: Option<(Range<usize>, Rect)>) {
        let Some((range, rect)) = hovered else {
            self.editor_hover = None;
            return;
        };
        // A different hover target may lie under the pointer while it travels
        // toward an already visible native tooltip. Do not let that target
        // replace the active payload during the handoff.
        if native_tooltip_handoff_blocks(ui.ctx(), rect.expand(theme::SPACE.tight)) {
            return;
        }
        let (Some(generation), Some(uri)) = (self.tinymist_generation, self.tinymist_uri.clone())
        else {
            self.editor_hover = None;
            return;
        };
        let version = revision_as_i32(self.document.revision);
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

        let should_request = self.preview.tinymist_lsp_ready
            && self.tinymist_current_open
            && self
                .editor_hover
                .as_ref()
                .is_some_and(|hover| !hover.requested);
        if should_request {
            let position = lsp_position_at_char(&self.document.source, range.start);
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
            let tooltip = HoverTooltipOverlay {
                origin: rect.expand(theme::SPACE.tight),
                anchor: rect.left_bottom() + egui::vec2(0.0, METRICS.editor.tooltip_gap),
                detail: detail.clone(),
                opacity,
            };
            // Semantic hover results use the same native viewport as toolbar
            // and diagnostic tooltips. Route them through the same handoff
            // gate so an editor token crossed en route cannot replace the
            // tooltip the pointer is approaching.
            if !native_tooltip_handoff_blocks(ui.ctx(), tooltip.origin) {
                ui.ctx()
                    .data_mut(|data| data.insert_temp(tooltip_id, tooltip));
            }
        }
    }

    fn diagnostic_targets_current_document(&self, diagnostic: &Diagnostic) -> bool {
        match &diagnostic.source {
            DiagnosticSource::Main => self.current_is_preview_document(),
            DiagnosticSource::File(path) => {
                self.document
                    .path
                    .as_ref()
                    .is_some_and(|current| same_path(current, path))
                    || (self.document.path.is_none()
                        && same_path(&self.tinymist_document_path(), path))
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
        let main_capture_pending = self.captures.has_pending_for("main");
        if main_capture_pending {
            if capture_preview_build_needed(
                main_capture_pending,
                !self.preview.pages.is_empty(),
                self.compile_deadline.is_some(),
                self.preview.status,
                self.preview.artifact_key.is_some(),
            ) {
                self.schedule_compile_now();
            }
            self.hide_webview();
            self.show_native_preview(ui);
            return;
        }

        if self.should_attempt_interactive_preview() {
            // The interactive viewer is a native child view, so it does not
            // inherit egui's clip rectangle. Keep its bounds inside the
            // preview pane or it can draw over the editor after a resize.
            let available = ui.available_rect_before_wrap();
            let clip = ui.clip_rect();
            let rect = clipped_preview_rect(available, clip);
            let native_rect = egui_rect_to_native(ui.ctx(), rect);
            trace_native_preview_bounds(ui.ctx(), available, clip, rect, native_rect);
            if self.update_webview(
                ui.ctx(),
                frame,
                rect,
                native_rect,
                ui.visuals().panel_fill,
                true,
            ) {
                ui.allocate_rect(rect, Sense::hover());
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
            let raster_is_current =
                self.raster_content_freshness() == Some(RasterContentFreshness::Current);
            let page_count =
                if snapshot_scene_hides_preview_pages(self.snapshot_scene) || !raster_is_current {
                    0
                } else {
                    self.preview.pages.len()
                };
            if header_width >= METRICS.preview.header_pages_min_width {
                if icon_button_enabled(
                    ui,
                    page_count > 0 && self.preview.visible_page > 0,
                    UiIcon::Previous,
                    "Previous page",
                )
                .clicked()
                {
                    self.preview.requested_page = Some(self.preview.visible_page - 1);
                }
                ui.label(if page_count == 0 {
                    "–/–".to_owned()
                } else {
                    format!("{}/{page_count}", self.preview.visible_page + 1)
                });
                if icon_button_enabled(
                    ui,
                    self.preview.visible_page + 1 < page_count,
                    UiIcon::Next,
                    "Next page",
                )
                .clicked()
                {
                    self.preview.requested_page = Some(self.preview.visible_page + 1);
                }
            }
            if header_width >= METRICS.preview.header_zoom_min_width {
                ui.separator();
                if icon_button(ui, UiIcon::ZoomOut, "Zoom out").clicked() {
                    self.preview.requested_zoom = Some(
                        (self.preview.zoom / METRICS.preview.zoom_step)
                            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                    );
                    self.preview.fit_width = false;
                }
                if icon_button(ui, UiIcon::ZoomIn, "Zoom in").clicked() {
                    self.preview.requested_zoom = Some(
                        (self.preview.zoom * METRICS.preview.zoom_step)
                            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                    );
                    self.preview.fit_width = false;
                }
                if header_width >= METRICS.preview.header_percent_min_width {
                    ui.label(format!("{:.0}%", self.preview.zoom * 100.0));
                }
                if icon_button(
                    ui,
                    UiIcon::FitWidth,
                    if self.preview.fit_width {
                        "Fit page width (on)"
                    } else {
                        "Fit page width"
                    },
                )
                .clicked()
                {
                    self.preview.fit_width = !self.preview.fit_width;
                }
            }
        }
    }

    fn show_native_preview(&mut self, ui: &mut egui::Ui) {
        let viewport_rect = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(viewport_rect, 0.0, preview_background(ui));
        if self.preview.pages.is_empty() || snapshot_scene_hides_preview_pages(self.snapshot_scene)
        {
            ui.centered_and_justified(|ui| match self.preview.status {
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
            .preview
            .pages
            .iter()
            .map(|page| page.size[0] as f32 * PDF_POINTS_PER_PREVIEW_PIXEL)
            .fold(1.0_f32, f32::max);
        if self.preview.fit_width {
            self.preview.zoom = ((viewport_rect.width() - PAGE_MARGIN * 2.0) / widest_page)
                .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM);
        }

        let scroll_id = ui.make_persistent_id("pdf-preview-scroll");
        let pointer = ui.ctx().input(|input| input.pointer.latest_pos());
        let pinch = ui.ctx().input(|input| input.zoom_delta());
        let pinch_active = pointer.is_some_and(|pointer| viewport_rect.contains(pointer))
            && (pinch - 1.0).abs() > 0.001;
        let old_zoom = self.preview.zoom;
        let new_zoom = if pinch_active {
            self.preview.fit_width = false;
            (self.preview.zoom * pinch).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM)
        } else {
            self.preview
                .requested_zoom
                .take()
                .unwrap_or(self.preview.zoom)
        };
        if (new_zoom - old_zoom).abs() > f32::EPSILON {
            let anchor = pointer
                .filter(|pointer| viewport_rect.contains(*pointer))
                .unwrap_or_else(|| viewport_rect.center());
            let mut state = egui::scroll_area::State::load(ui.ctx(), scroll_id).unwrap_or_default();
            state.offset =
                zoom_anchored_offset(state.offset, anchor - viewport_rect.min, old_zoom, new_zoom);
            state.store(ui.ctx(), scroll_id);
            self.preview.zoom = new_zoom;
            self.preview.fit_width = false;
        }

        let geometries = page_stack_geometry(
            self.preview.pages.iter().map(|page| page.size),
            self.preview.zoom,
        );
        let content_width = geometries
            .iter()
            .map(|page| page.size.x)
            .fold(viewport_rect.width(), f32::max)
            + PAGE_MARGIN * 2.0;
        let content_height = stack_height(&geometries);
        let requested_page = self.preview.requested_page.take();
        let mut clicked_link = None;
        let raster_is_current =
            self.raster_content_freshness() == Some(RasterContentFreshness::Current);

        let output = egui::ScrollArea::both()
            .id_salt("pdf-preview-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_size(Vec2::new(content_width, content_height));
                let page_theme = theme::preview_palette(self.preview.dark);
                for (page, geometry) in self.preview.pages.iter().zip(&geometries) {
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
                        if !raster_is_current {
                            continue;
                        }
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
        self.preview.visible_page = visible_page(
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
            ui.label(RichText::new("Problems").strong());
            ui.label(
                RichText::new(format!(
                    "{} diagnostics",
                    self.preview.diagnostics.len() + self.preview.tinymist_diagnostics.len()
                ))
                .size(theme::TYPE.supporting)
                .color(ui.visuals().weak_text_color()),
            );
        });
        ui.separator();
        let diagnostic_count =
            self.preview.diagnostics.len() + self.preview.tinymist_diagnostics.len();
        let mut jump_target = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if diagnostic_count == 0 {
                    ui.label(RichText::new("No compiler diagnostics").weak());
                }
                for (index, diagnostic) in self
                    .preview
                    .diagnostics
                    .iter()
                    .chain(&self.preview.tinymist_diagnostics)
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
                                    ui.label(
                                        RichText::new(detail)
                                            .size(theme::TYPE.supporting)
                                            .monospace()
                                            .weak(),
                                    );
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
                if !self.preview.raw_diagnostics.is_empty() {
                    ui.collapsing("Raw CLI stdout", |ui| {
                        ui.label(RichText::new(&self.preview.raw_diagnostics).monospace());
                    });
                }
            });
        if let Some(diagnostic) = jump_target {
            self.jump_to_diagnostic(diagnostic);
        }
    }

    fn record_status_transition(&mut self) {
        if self.recorded_status == Some(self.preview.status) {
            return;
        }
        let kind = match self.preview.status {
            PreviewStatus::Ready(_) => NoticeKind::Success,
            PreviewStatus::Error => NoticeKind::Error,
            PreviewStatus::Waiting | PreviewStatus::Compiling => NoticeKind::Info,
        };
        self.push_status_log(self.status_detail(), kind);
        self.recorded_status = Some(self.preview.status);
    }

    fn record_notice_transition(&mut self) {
        let Some(notice) = self.notice.clone() else {
            self.recorded_notice = None;
            return;
        };
        if self.recorded_notice.as_ref() == Some(&notice) {
            return;
        }
        self.push_status_log(notice.message.clone(), notice.kind);
        self.recorded_notice = Some(notice);
    }

    fn push_status_log(&mut self, detail: String, kind: NoticeKind) {
        push_status_log_entry(
            &mut self.status_log,
            StatusLogEntry {
                timestamp: current_timestamp(),
                detail,
                kind,
            },
        );
    }

    fn document_extension_label(&self) -> Option<String> {
        if self.document.kind.is_typst() {
            return None;
        }
        self.document
            .path
            .as_ref()
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
            .or_else(|| {
                Some(
                    match self.document.kind {
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
        if !self.document.kind.is_editable() {
            return None;
        }
        let char_index = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .and_then(|state| state.cursor.char_range())
            .map_or(0, |range| range.primary.index.0)
            .min(self.document.source.chars().count());
        Some(line_column_at_char(&self.document.source, char_index))
    }

    fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        self.prepare_editor_source_data();
        let source_metrics = self.editor_data.source_metrics();
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
            if self.document.kind.is_editable() {
                // This is intentionally the first right-to-left item: its
                // position is pinned to the window edge regardless of status
                // or notice length.
                ui.label(
                    RichText::new(format!(
                        "{} lines · {} chars",
                        source_metrics.line_count, source_metrics.char_count
                    ))
                    .size(theme::TYPE.supporting),
                );
            }
            if let Some((line, column)) = self.cursor_coordinates(ui.ctx()) {
                ui.separator();
                native_hover_text(
                    ui.label(
                        RichText::new(format!("Ln {line}, Col {column}"))
                            .size(theme::TYPE.supporting)
                            .color(ui.visuals().weak_text_color()),
                    ),
                    "Cursor position",
                );
            }

            let dark_mode = ui.visuals().dark_mode;
            let (icon, color, timing) =
                if !self.may_run_compilation() && self.typst_preview_available() {
                    (UiIcon::Waiting, neutral_color(dark_mode), None)
                } else {
                    match self.preview.status {
                        PreviewStatus::Waiting => (UiIcon::Waiting, neutral_color(dark_mode), None),
                        PreviewStatus::Compiling => (UiIcon::Refresh, info_color(dark_mode), None),
                        PreviewStatus::Ready(elapsed) => (
                            UiIcon::Check,
                            success_color(dark_mode),
                            self.document
                                .kind
                                .is_typst()
                                .then(|| format!("{:.0} ms", elapsed.as_secs_f64() * 1000.0)),
                        ),
                        PreviewStatus::Error => (UiIcon::Warning, error_color(dark_mode), None),
                    }
                };
            let status_detail = self.status_detail();
            let status_response = ui
                .horizontal(|ui| {
                    if matches!(self.preview.status, PreviewStatus::Ready(_))
                        && let Some(extension) = self.document_extension_label()
                    {
                        ui.label(
                            RichText::new(extension)
                                .size(theme::TYPE.supporting)
                                .strong()
                                .color(color),
                        );
                    } else {
                        static_icon(ui, icon, color);
                    }
                    if let Some(timing) = timing {
                        ui.label(
                            RichText::new(timing)
                                .size(theme::TYPE.supporting)
                                .strong()
                                .color(color),
                        );
                    }
                })
                .response
                .interact(Sense::click());
            native_hover_text(
                status_response.clone(),
                format!("{status_detail}\nDouble-click for recent status"),
            );
            if status_response.double_clicked() {
                self.open_app_popup(AppPopup::StatusLog {
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
                            .size(theme::TYPE.supporting)
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
                        egui::Label::new(
                            RichText::new(&notice.message)
                                .size(theme::TYPE.supporting)
                                .color(color),
                        )
                        .truncate()
                        .halign(Align::RIGHT),
                    ),
                    &notice.message,
                );
            }
        });
    }

    fn status_detail(&self) -> String {
        if !self.may_run_compilation() && self.typst_preview_available() {
            return "Automatic preview updates paused".to_owned();
        }
        if !self.typst_preview_available() {
            return match (self.document.kind, self.preview.status) {
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
        match self.preview.status {
            PreviewStatus::Waiting => "PDF build queued".to_owned(),
            PreviewStatus::Compiling => "Compiling PDF".to_owned(),
            PreviewStatus::Ready(elapsed) => {
                format!("PDF ready in {:.0} ms", elapsed.as_secs_f64() * 1000.0)
            }
            PreviewStatus::Error => self
                .preview
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
        _rect: Rect,
        native_rect: Rect,
        background: Color32,
        visible: bool,
    ) -> bool {
        use wry::dpi::{LogicalPosition, LogicalSize};

        let Some(url) = self.preview.interactive_url.clone() else {
            self.hide_webview();
            return false;
        };
        let navigation_state = PreviewNavigationContext {
            base_url: url.clone(),
            project_root: self.project_root(),
            source_dir: self.preview_document_path().parent().map(Path::to_path_buf),
        };
        if let Some(shared) = &self.webview_navigation
            && let Ok(mut current) = shared.lock()
        {
            *current = navigation_state.clone();
        }
        let bounds = wry::Rect {
            position: LogicalPosition::new(native_rect.left() as f64, native_rect.top() as f64)
                .into(),
            size: LogicalSize::new(
                native_rect.width().max(1.0) as f64,
                native_rect.height().max(1.0) as f64,
            )
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
                self.preview.webview_state = ServiceState::Starting(
                    "Interactive preview will resume when the window is active".to_owned(),
                );
                return false;
            }
            let navigation_sender = self.web_link_sender.clone();
            let navigation_repaint = context.clone();
            let popup_sender = self.web_link_sender.clone();
            let popup_repaint = context.clone();
            let shared_navigation = Arc::new(Mutex::new(navigation_state));
            let navigation_handler_state = Arc::clone(&shared_navigation);
            let popup_handler_state = Arc::clone(&shared_navigation);
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
                    let action = navigation_handler_state
                        .lock()
                        .map(|state| preview_navigation_action(&state, &candidate))
                        .unwrap_or_else(|_| PreviewNavigationAction::Dispatch(candidate));
                    match action {
                        PreviewNavigationAction::Embed => true,
                        PreviewNavigationAction::Dispatch(target) => {
                            if navigation_sender.send(target).is_ok() {
                                navigation_repaint.request_repaint();
                            }
                            false
                        }
                    }
                })
                .with_new_window_req_handler(move |candidate, _features| {
                    let target = popup_handler_state
                        .lock()
                        .ok()
                        .and_then(|state| preview_new_window_target(&state, &candidate));
                    if let Some(target) = target
                        && popup_sender.send(target).is_ok()
                    {
                        popup_repaint.request_repaint();
                    }
                    wry::NewWindowResponse::Deny
                });
            let built = if self.window_host.is_root() {
                let Some(window) = frame.winit_window() else {
                    self.preview.webview_state = ServiceState::Degraded(
                        "The native window handle is temporarily unavailable".to_owned(),
                    );
                    return false;
                };
                builder.build_as_child(window.as_ref())
            } else {
                let Some(window) = self.native_window_parent.as_ref() else {
                    self.preview.webview_state = ServiceState::Starting(
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
                    self.webview_reload_pending = false;
                    self.webview_navigation = Some(shared_navigation);
                    self.preview.webview_state =
                        ServiceState::Ready("Tinymist vector frontend is embedded".to_owned());
                }
                Err(error) => {
                    self.fail_local_webview(format!(
                        "Could not embed the Tinymist preview: {error}"
                    ));
                    return false;
                }
            }
        }
        if webview_navigation_required(
            self.webview_url.as_deref(),
            &url,
            self.webview_reload_pending,
        ) {
            if let Some(webview) = &self.webview
                && let Err(error) = webview.load_url(&url)
            {
                self.fail_local_webview(format!("Could not load the Tinymist preview: {error}"));
                return false;
            }
            self.webview_url = Some(url);
            self.webview_reload_pending = false;
        }
        if let Some(webview) = &self.webview {
            let _ = webview.set_background_color((
                background.r(),
                background.g(),
                background.b(),
                background.a(),
            ));
            if let Err(error) = webview.set_bounds(bounds) {
                self.fail_local_webview(format!("Could not position the vector preview: {error}"));
                return false;
            }
            if let Err(error) = webview.set_visible(visible) {
                self.fail_local_webview(format!("Could not show the vector preview: {error}"));
                return false;
            }
        }
        if self.preview.tinymist_state.is_ready() {
            self.preview.webview_state =
                ServiceState::Ready("Tinymist vector frontend is embedded".to_owned());
        }
        true
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn update_webview(
        &mut self,
        _context: &egui::Context,
        _frame: &mut eframe::Frame,
        _rect: Rect,
        _native_rect: Rect,
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
            self.webview_reload_pending = false;
            self.webview_navigation = None;
            self.preview.webview_state =
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
            self.autosave_deadline = (self.settings.auto_save
                && self.document.path.is_some()
                && self.is_dirty())
            .then(|| {
                Instant::now() + Duration::from_millis(self.settings.auto_save_delay_ms.max(100))
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
        if self.snapshot_scene.is_some() {
            return;
        }
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
            if self.document_workflow.modal_suspended {
                self.document_workflow.modal_had_focus = false;
                self.document_workflow.modal_suspended = false;
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
        let interaction_id = tooltip_interaction_id(&context);
        let tooltip_retained = native_tooltip_handoff_active(&context, true);
        if !tooltip_retained {
            self.diagnostic_tooltip = None;
            context.data_mut(|data| {
                data.remove::<HoverTooltipOverlay>(native_tooltip_id);
                data.remove::<TooltipGeometry>(geometry_id);
                data.remove::<TooltipInteractionState>(interaction_id);
            });
        }
        self.process_open_requests(&context);
        self.process_native_menu_commands(&context, frame);
        self.execute_pending_app_popup_action(&context, frame);
        self.execute_pending_document_action(&context, frame);
        self.poll_font_catalog(&context);
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
        self.record_notice_transition();

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
        let designated_preview = self.designated_preview_path().is_some();
        if self.document.kind.preview_only() {
            // This presentation override intentionally does not mutate
            // `view_mode`: returning to a source file restores the user's Code,
            // Split, or Preview preference.
            egui::CentralPanel::default()
                .frame(theme::content_panel_frame(ui.style()))
                .show(ui, |ui| self.show_preview(ui, frame));
        } else if self.document.kind == DocumentKind::Text && !designated_preview {
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
                    let layout = theme::split_pane_layout(ui.available_width());
                    egui::Panel::left("editor")
                        .frame(theme::content_panel_frame(ui.style()))
                        .resizable(true)
                        .default_size(layout.editor_width)
                        .min_size(layout.editor_minimum)
                        .max_size(layout.editor_maximum)
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
    key: ArtifactKey,
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
        format!("preview-{}-{}-{index}-{dark}", key.revision, key.generation),
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
) -> Option<PendingExport> {
    pending
        .take()
        .filter(|export| export.document_epoch == document_epoch)
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

#[cfg(test)]
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
    dark_mode: bool,
) {
    for node in nodes {
        let is_active = active.is_some_and(|path| path == node.path);
        let is_preview = preview.is_some_and(|path| path == node.path);
        let label = node.display_name().into_owned();
        if node.is_directory() {
            let color = workspace_entry_color(&node.path, true, false, dark_mode);
            let open = builder.node(
                NodeBuilder::dir(node.path.clone())
                    .default_open(active.is_some_and(|path| path.starts_with(&node.path)))
                    .icon(|ui| paint_tree_icon(ui, true))
                    .label_ui(move |ui| {
                        let text = RichText::new(&label).color(color);
                        ui.add(workspace_entry_label(text, is_active));
                    }),
            );
            if open {
                add_workspace_nodes(builder, &node.children, active, preview, dark_mode);
            }
            builder.close_dir();
        } else if node.is_file() {
            let color = workspace_entry_color(&node.path, false, false, dark_mode);
            builder.node(
                NodeBuilder::leaf(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, false))
                    .label_ui(move |ui| {
                        ui.horizontal(|ui| {
                            let text = RichText::new(&label).color(color);
                            ui.add(workspace_entry_label(text, is_active));
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
            let color = workspace_entry_color(&node.path, false, true, dark_mode);
            builder.node(
                NodeBuilder::leaf(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, false))
                    .label_ui(move |ui| {
                        let text = RichText::new(format!("{label} (link)")).color(color);
                        ui.add(workspace_entry_label(text, is_active));
                    }),
            );
        }
    }
}

fn workspace_entry_label(text: RichText, is_active: bool) -> egui::Label {
    let text = if is_active {
        text.font(theme::strong_ui_font()).strong()
    } else {
        text
    };
    theme::nonselectable_label(text)
}

fn workspace_entry_color(path: &Path, directory: bool, symlink: bool, dark_mode: bool) -> Color32 {
    if directory {
        return theme::palette(dark_mode).accent;
    }
    if symlink {
        return theme::syntax_palette(dark_mode).comment;
    }
    let syntax = theme::syntax_palette(dark_mode);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("typ") => syntax.keyword,
        Some("pdf") | Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("webp")
        | Some("bmp") | Some("ico") | Some("tif") | Some("tiff") => theme::palette(dark_mode).info,
        Some("txt") | Some("md") | Some("markdown") | Some("json") | Some("jsonc")
        | Some("toml") | Some("yaml") | Some("yml") | Some("xml") | Some("html") | Some("htm")
        | Some("css") | Some("scss") | Some("js") | Some("jsx") | Some("ts") | Some("tsx")
        | Some("rs") | Some("py") | Some("rb") | Some("go") | Some("java") | Some("c")
        | Some("h") | Some("cc") | Some("cpp") | Some("hpp") | Some("sh") | Some("bash")
        | Some("zsh") | Some("fish") | Some("sql") | Some("csv") | Some("tsv") | Some("ini")
        | Some("cfg") | Some("conf") | Some("log") | Some("tex") | Some("bib") => syntax.plain,
        _ => theme::syntax_palette(dark_mode).comment,
    }
}

const EXPLORER_SECTION_SPECS: [(&str, bool); 6] = [
    ("workspace-files", true),
    ("workspace-contents", true),
    ("workspace-subfiles", false),
    ("workspace-symbols", false),
    ("workspace-packages", false),
    ("workspace-references", false),
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
                ui.label(
                    RichText::new("No headings")
                        .size(theme::TYPE.supporting)
                        .weak(),
                );
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
                ui.label(
                    RichText::new("No included files")
                        .size(theme::TYPE.supporting)
                        .weak(),
                );
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
                ui.label(
                    RichText::new("No definitions or functions")
                        .size(theme::TYPE.supporting)
                        .weak(),
                );
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
                ui.label(
                    RichText::new("No packages")
                        .size(theme::TYPE.supporting)
                        .weak(),
                );
            } else {
                for package in &index.packages {
                    explorer_index_row(ui, package, None, 0.0);
                }
            }
        },
    );

    explorer_section(
        ui,
        "workspace-references",
        "Tags and references",
        false,
        body_height,
        |ui| {
            if index.references.is_empty() {
                ui.label(
                    RichText::new("No tags or references")
                        .size(theme::TYPE.supporting)
                        .weak(),
                );
                return;
            }
            for reference in &index.references {
                let location = format!(
                    "{}:{}",
                    project_relative_path(root, &reference.path),
                    reference.line
                );
                let response = explorer_index_row(
                    ui,
                    &reference.label,
                    Some(&reference.line.to_string()),
                    0.0,
                );
                if native_hover_text(response, location).clicked() {
                    *target = Some((reference.path.clone(), reference.line));
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

struct RefreshIconGeometry {
    arc: Vec<Pos2>,
    shaft: [Pos2; 2],
    wing: [Pos2; 2],
}

fn refresh_icon_geometry(rect: Rect) -> RefreshIconGeometry {
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.34;
    let start_angle = 0.55_f32;
    let end_angle = 5.55_f32;
    let arc = (0..=20)
        .map(|index| {
            let angle = start_angle + (end_angle - start_angle) * index as f32 / 20.0;
            center + Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect::<Vec<_>>();
    let junction = *arc.last().expect("refresh arc has an endpoint");
    let tangent = Vec2::new(-end_angle.sin(), end_angle.cos()).normalized();
    let radial = Vec2::new(end_angle.cos(), end_angle.sin()).normalized();
    let tip = junction + tangent * 2.1;
    let outer_corner = tip - tangent * 2.2 + radial * 1.5;
    RefreshIconGeometry {
        arc,
        shaft: [junction, tip],
        wing: [tip, outer_corner],
    }
}

struct EyeIconGeometry {
    upper: [Pos2; 4],
    lower: [Pos2; 4],
    pupil_radius: f32,
}

fn eye_icon_geometry(rect: Rect) -> EyeIconGeometry {
    let center = rect.center();
    let half_width = rect.width() * 0.44;
    let control_lift = rect.height() * 0.34;
    let control_inset = half_width * 0.48;
    let left = Pos2::new(center.x - half_width, center.y);
    let right = Pos2::new(center.x + half_width, center.y);
    EyeIconGeometry {
        upper: [
            left,
            Pos2::new(center.x - control_inset, center.y - control_lift),
            Pos2::new(center.x + control_inset, center.y - control_lift),
            right,
        ],
        lower: [
            left,
            Pos2::new(center.x - control_inset, center.y + control_lift),
            Pos2::new(center.x + control_inset, center.y + control_lift),
            right,
        ],
        pupil_radius: rect.width().min(rect.height()) * 0.11,
    }
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
            // Continue the arc into one side of the arrowhead, then place its
            // outer wing beyond the circle. This keeps the small glyph open
            // instead of layering a large chevron over its own body.
            let geometry = refresh_icon_geometry(rect);
            painter.add(egui::Shape::line(geometry.arc, stroke));
            painter.line_segment(geometry.shaft, stroke);
            painter.line_segment(geometry.wing, stroke);
        }
        UiIcon::Waiting => {
            painter.circle_stroke(center, rect.width().min(rect.height()) * 0.36, stroke);
        }
        UiIcon::Eye => {
            let geometry = eye_icon_geometry(rect);
            painter.add(egui::Shape::CubicBezier(
                egui::epaint::CubicBezierShape::from_points_stroke(
                    geometry.upper,
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ),
            ));
            painter.add(egui::Shape::CubicBezier(
                egui::epaint::CubicBezierShape::from_points_stroke(
                    geometry.lower,
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ),
            ));
            painter.circle_filled(center, geometry.pupil_radius, color);
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

fn clipped_preview_rect(available: Rect, clip: Rect) -> Rect {
    available.intersect(clip)
}

fn trace_native_preview_bounds(
    context: &egui::Context,
    available: Rect,
    clip: Rect,
    egui_rect: Rect,
    native_rect: Rect,
) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("TIPTOPTYP_UI_TRACE").is_some()) {
        return;
    }

    let (viewport, egui_pixels_per_point, native_pixels_per_point) = context.input(|input| {
        (
            input.viewport_rect(),
            input.pixels_per_point,
            input.viewport().native_pixels_per_point,
        )
    });
    let native_pixels_per_point = native_pixels_per_point.unwrap_or(egui_pixels_per_point);
    let zoom = egui_pixels_per_point / native_pixels_per_point;
    eprintln!(
        "ui.preview.bounds available={} clip={} egui={} native={} viewport={} zoom={zoom:.3} egui_ppp={egui_pixels_per_point:.3} native_ppp={native_pixels_per_point:.3}",
        format_rect(available),
        format_rect(clip),
        format_rect(egui_rect),
        format_rect(native_rect),
        format_rect(viewport),
    );
}

fn format_rect(rect: Rect) -> String {
    format!(
        "({:.1},{:.1})-({:.1},{:.1})",
        rect.left(),
        rect.top(),
        rect.right(),
        rect.bottom()
    )
}

fn egui_rect_to_native(context: &egui::Context, rect: Rect) -> Rect {
    let (viewport, egui_pixels_per_point, native_pixels_per_point) = context.input(|input| {
        (
            input.viewport_rect(),
            input.pixels_per_point,
            input.viewport().native_pixels_per_point,
        )
    });
    let scale = (egui_pixels_per_point / native_pixels_per_point.unwrap_or(egui_pixels_per_point))
        .max(0.01);
    scale_rect_from_egui_to_native(rect, viewport, scale)
}

fn scale_rect_from_egui_to_native(rect: Rect, viewport: Rect, scale: f32) -> Rect {
    Rect::from_min_max(
        viewport.min + (rect.min - viewport.min) * scale,
        viewport.min + (rect.max - viewport.min) * scale,
    )
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
            ui.label(
                RichText::new("Updating preview…")
                    .size(theme::TYPE.supporting)
                    .weak(),
            );
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
fn preview_navigation_action(
    context: &PreviewNavigationContext,
    candidate: &str,
) -> PreviewNavigationAction {
    if candidate == "about:blank" {
        return PreviewNavigationAction::Embed;
    }
    if same_web_origin(&context.base_url, candidate) {
        return preview_document_file_url(
            candidate,
            &context.project_root,
            context.source_dir.as_deref(),
        )
        .map_or(
            PreviewNavigationAction::Embed,
            PreviewNavigationAction::Dispatch,
        );
    }
    PreviewNavigationAction::Dispatch(candidate.to_owned())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn preview_new_window_target(
    context: &PreviewNavigationContext,
    candidate: &str,
) -> Option<String> {
    match preview_navigation_action(context, candidate) {
        PreviewNavigationAction::Dispatch(target) => Some(target),
        PreviewNavigationAction::Embed => None,
    }
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

fn apply_ui_scale(context: &egui::Context, percent: u16) {
    context.set_zoom_factor(ui_scale_factor(percent));
}

fn ui_scale_factor(percent: u16) -> f32 {
    f32::from(percent.clamp(75, 150)) / f32::from(DEFAULT_UI_SCALE_PERCENT)
}

fn find_step_for_enter(enter_pressed: bool, shift: bool) -> Option<FindStep> {
    enter_pressed.then_some(if shift {
        FindStep::Previous
    } else {
        FindStep::Next
    })
}

fn current_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    utc_timestamp_from_unix_seconds(seconds)
}

fn utc_timestamp_from_unix_seconds(seconds: u64) -> String {
    let day = 24 * 60 * 60;
    let seconds = seconds % day;
    format!(
        "{:02}:{:02}:{:02}Z",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum FontPickerSelection {
    Default,
    Editor,
    Family {
        name: String,
        path: String,
        face_index: u32,
    },
}

fn show_font_family_picker(
    ui: &mut egui::Ui,
    id: &'static str,
    catalog: &FontCatalog,
    selected_path: Option<&str>,
    selected_family: Option<&str>,
    default_label: &'static str,
    offer_editor_font: bool,
) -> Option<FontPickerSelection> {
    let selected_text = selected_family
        .map(str::to_owned)
        .or_else(|| {
            selected_path.and_then(|path| {
                Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
        })
        .unwrap_or_else(|| default_label.to_owned());
    let mut selection = None;
    egui::ComboBox::from_id_salt(id)
        .width(230.0)
        .height(350.0)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            if offer_editor_font {
                if ui
                    .selectable_label(
                        selected_path.is_none() && default_label == "System UI",
                        "System UI",
                    )
                    .clicked()
                {
                    selection = Some(FontPickerSelection::Default);
                }
                if ui
                    .selectable_label(
                        selected_path.is_none() && default_label == "Editor font",
                        "Editor font",
                    )
                    .clicked()
                {
                    selection = Some(FontPickerSelection::Editor);
                }
            } else if ui
                .selectable_label(selected_path.is_none(), default_label)
                .clicked()
            {
                selection = Some(FontPickerSelection::Default);
            }
            let mut previous_origin = None;
            for family in catalog.families() {
                if previous_origin != Some(family.origin) {
                    ui.separator();
                    ui.label(RichText::new(family.origin.label()).strong());
                    previous_origin = Some(family.origin);
                }
                let is_selected = selected_family
                    .is_some_and(|selected| selected.eq_ignore_ascii_case(&family.name))
                    && selected_path.is_some_and(|path| family.contains_path(Path::new(path)));
                if ui.selectable_label(is_selected, &family.name).clicked()
                    && let Some(face) = family.primary_face()
                {
                    selection = Some(FontPickerSelection::Family {
                        name: family.name.clone(),
                        path: face.path.display().to_string(),
                        face_index: face.index,
                    });
                }
            }
        });
    selection
}

fn update_staged_font_weight(
    committed: &mut u16,
    staged: &mut Option<u16>,
    displayed: u16,
    changed: bool,
    pointer_down: bool,
    drag_stopped: bool,
) {
    if drag_stopped {
        *committed = if changed {
            displayed
        } else {
            staged.take().unwrap_or(displayed)
        };
        *staged = None;
    } else if changed && pointer_down {
        *staged = Some(displayed);
    } else if changed {
        *committed = displayed;
        *staged = None;
    }
}

fn show_font_weight_control(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    weight: &mut u16,
    staged_weight: &mut Option<u16>,
    support: Option<&theme::FontWeightSupport>,
) {
    let Some(support) = support else {
        ui.label(RichText::new("Static weight").weak());
        return;
    };
    ui.label(RichText::new("Weight").strong());
    match support {
        theme::FontWeightSupport::Continuous { min, max, .. } => {
            *weight = (*weight).clamp(*min, *max);
            let mut displayed = staged_weight.unwrap_or(*weight).clamp(*min, *max);
            let response = ui.add_sized(
                [
                    METRICS.settings.ui_font_weight_width,
                    ui.spacing().interact_size.y,
                ],
                egui::Slider::new(&mut displayed, *min..=*max)
                    .clamping(egui::SliderClamping::Always),
            );
            update_staged_font_weight(
                weight,
                staged_weight,
                displayed,
                response.changed(),
                response.is_pointer_button_down_on() || response.dragged(),
                response.drag_stopped(),
            );
        }
        theme::FontWeightSupport::Discrete { values, .. } => {
            *staged_weight = None;
            *weight = support.clamp(*weight);
            ui.push_id(id_salt, |ui| {
                egui::ComboBox::from_id_salt("font-weight")
                    .selected_text(weight.to_string())
                    .show_ui(ui, |ui| {
                        for value in values {
                            ui.selectable_value(weight, *value, value.to_string());
                        }
                    });
            });
        }
    }
    if ui.small_button("Reset").clicked() {
        *weight = support.default_weight();
        *staged_weight = None;
    }
}

fn show_typst_override_editor(
    ui: &mut egui::Ui,
    overrides: &mut TypstStyleOverrides,
    palette: theme::SyntaxPalette,
    syntect_theme: &syntect::highlighting::Theme,
    weight_support: &theme::FontWeightSupport,
) {
    for group in ["Markup", "Math", "Code", "Diagnostics"] {
        ui.label(RichText::new(group).strong());
        egui::Grid::new(("typst-override-grid", group))
            .num_columns(9)
            .striped(true)
            .spacing(egui::vec2(theme::SPACE.control, theme::SPACE.tight))
            .show(ui, |ui| {
                ui.add_sized(
                    [
                        METRICS.settings.override_role_width,
                        METRICS.settings.override_row_height,
                    ],
                    egui::Label::new(RichText::new("Syntax").size(theme::TYPE.supporting).weak()),
                );
                for (label, width) in [
                    ("Foreground", METRICS.settings.override_color_width),
                    ("Background", METRICS.settings.override_color_width),
                    ("Weight", METRICS.settings.override_weight_width),
                    ("Italic", METRICS.settings.override_decoration_width),
                    ("Underline", METRICS.settings.override_decoration_width),
                    ("Strike", METRICS.settings.override_decoration_width),
                ] {
                    ui.add_sized(
                        [width, METRICS.settings.override_row_height],
                        egui::Label::new(RichText::new(label).size(theme::TYPE.supporting).weak())
                            .truncate(),
                    );
                }
                ui.add_sized(
                    [
                        METRICS.settings.override_sample_width,
                        METRICS.settings.override_row_height,
                    ],
                    egui::Label::new(
                        RichText::new("Live sample")
                            .size(theme::TYPE.supporting)
                            .weak(),
                    ),
                );
                ui.add_sized(
                    [
                        METRICS.settings.override_reset_width,
                        METRICS.settings.override_row_height,
                    ],
                    egui::Label::new(""),
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
                    ui.add_sized(
                        [
                            METRICS.settings.override_role_width,
                            METRICS.settings.override_row_height,
                        ],
                        egui::Label::new(role.label()).truncate(),
                    );
                    optional_color_override(
                        ui,
                        (role, "foreground"),
                        &mut style_override.foreground,
                        inherited.foreground,
                    );
                    optional_color_override(
                        ui,
                        (role, "background"),
                        &mut style_override.background,
                        inherited.background,
                    );
                    optional_weight_override(
                        ui,
                        (role, "weight"),
                        &mut style_override.weight,
                        inherited.weight,
                        weight_support,
                    );
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
                    ui.add_sized(
                        [
                            METRICS.settings.override_sample_width,
                            METRICS.settings.override_row_height,
                        ],
                        egui::Label::new(sample).truncate(),
                    );

                    if ui
                        .add_sized(
                            [
                                METRICS.settings.override_reset_width,
                                METRICS.settings.override_row_height,
                            ],
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
) {
    ui.push_id(id, |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(
                METRICS.settings.override_color_width,
                METRICS.settings.override_row_height,
            ),
            Layout::left_to_right(Align::Center),
            |ui| {
                theme::apply_compact_control_spacing(ui);
                let mut color = value.map_or(inherited, color_from_rgba);
                let response = ui.color_edit_button_srgba(&mut color);
                if response.changed() {
                    *value = Some(rgba_from_color(color));
                }
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
    let state = typst_override_state(*value);
    if ui
        .push_id(id, |ui| {
            typst_overrides_hover_text(
                ui.add_sized(
                    [
                        METRICS.settings.override_decoration_width,
                        METRICS.settings.override_row_height,
                    ],
                    egui::Button::new(format!("{label} {state}")),
                ),
                typst_override_toggle_tooltip(label, *value),
            )
        })
        .inner
        .clicked()
    {
        *value = next_typst_override_state(*value);
    }
}

fn optional_weight_override(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut Option<u16>,
    inherited: u16,
    support: &theme::FontWeightSupport,
) {
    ui.push_id(id, |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(
                METRICS.settings.override_weight_width,
                METRICS.settings.override_row_height,
            ),
            Layout::left_to_right(Align::Center),
            |ui| {
                egui::ComboBox::from_id_salt("value")
                    .width(METRICS.settings.override_weight_width)
                    .selected_text(
                        value.map_or_else(|| format!("inherit · {inherited}"), |it| it.to_string()),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(value, None, format!("Inherit · {inherited}"));
                        match support {
                            theme::FontWeightSupport::Continuous { min, max, .. } => {
                                for weight in theme::EDITOR_FONT_WEIGHTS
                                    .into_iter()
                                    .filter(|weight| weight >= min && weight <= max)
                                {
                                    ui.selectable_value(value, Some(weight), weight.to_string());
                                }
                            }
                            theme::FontWeightSupport::Discrete { values, .. } => {
                                for weight in values {
                                    ui.selectable_value(value, Some(*weight), weight.to_string());
                                }
                            }
                        }
                    });
            },
        );
    });
}

fn typst_override_state(value: Option<bool>) -> &'static str {
    match value {
        None => "inherit",
        Some(true) => "on",
        Some(false) => "off",
    }
}

fn next_typst_override_state(value: Option<bool>) -> Option<bool> {
    match value {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

fn typst_override_toggle_tooltip(label: &str, value: Option<bool>) -> String {
    format!("{label}: {}; click to cycle", typst_override_state(value))
}

fn rgba_from_color(color: Color32) -> Rgba {
    let [red, green, blue, alpha] = color.to_srgba_unmultiplied();
    Rgba::from_rgba(red, green, blue, alpha)
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
            ui.label(
                RichText::new(origin_label)
                    .size(theme::TYPE.supporting)
                    .strong()
                    .color(color),
            );
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
                        .size(theme::TYPE.supporting)
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
        ui.label(
            RichText::new(label)
                .size(theme::TYPE.supporting)
                .strong()
                .color(color),
        );
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
                ui.label(
                    RichText::new(status)
                        .size(theme::TYPE.supporting)
                        .strong()
                        .color(color),
                );
            });
        })
        .response;
    settings_hover_text(response, detail);
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

fn source_editor_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-source-editor")
}

fn native_hover_tooltip_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-native-hover-tooltip")
}

fn tooltip_geometry_id(context: &egui::Context) -> egui::Id {
    native_hover_tooltip_id(context).with("geometry")
}

fn tooltip_interaction_id(context: &egui::Context) -> egui::Id {
    native_hover_tooltip_id(context).with("interaction")
}

fn native_tooltip_handoff_active(context: &egui::Context, emit_trace: bool) -> bool {
    let geometry_id = tooltip_geometry_id(context);
    let interaction_id = tooltip_interaction_id(context);
    let pointer = context
        .pointer_hover_pos()
        .or_else(|| context.pointer_latest_pos());
    let now = context.input(|input| input.time);
    let (active, geometry, interaction) = context.data_mut(|data| {
        let mut geometry = data.get_temp::<TooltipGeometry>(geometry_id);
        if let Some(current) = geometry {
            let current = refresh_tooltip_root_geometry(current, pointer, now);
            geometry = Some(current);
            data.insert_temp(geometry_id, current);
        }
        let interaction = tooltip_interaction_for_geometry(
            geometry,
            data.get_temp::<TooltipInteractionState>(interaction_id),
        );
        let active = tooltip_handoff_is_active(pointer, now, geometry, interaction);
        (active, geometry, interaction)
    });
    if emit_trace {
        trace_native_tooltip_handoff(pointer, now, geometry, interaction, active);
    }
    if let Some(geometry) = geometry
        && geometry.handoff_until > now
        && !geometry.pointer_inside_viewport
        && !interaction.is_some_and(|state| state.focused || state.focus_requested)
        && !pointer
            .is_some_and(|pointer| tooltip_region_contains(pointer, geometry.origin, geometry.card))
    {
        // A pointer leaving the route may not generate another repaint. Make
        // the grace deadline self-expiring so the tooltip cannot linger
        // forever when there is no competing animation to drive the frame.
        context.request_repaint_after(Duration::from_secs_f64(
            (geometry.handoff_until - now).max(0.001),
        ));
    }
    active
}

fn native_tooltip_handoff_blocks(context: &egui::Context, candidate_origin: Rect) -> bool {
    let active = native_tooltip_handoff_active(context, false);
    let active_origin = context.data(|data| {
        data.get_temp::<TooltipGeometry>(tooltip_geometry_id(context))
            .map(|geometry| geometry.origin)
    });
    tooltip_handoff_blocks(active, active_origin, candidate_origin)
}

fn tooltip_handoff_blocks(
    active: bool,
    active_origin: Option<Rect>,
    candidate_origin: Rect,
) -> bool {
    // Focus state can briefly outlive its geometry while native viewports are
    // being recreated. Without a concrete source rectangle there is no route
    // to protect, so that stale state must not suppress every future tooltip.
    active && active_origin.is_some_and(|origin| origin != candidate_origin)
}

fn tooltip_handoff_is_active(
    pointer: Option<Pos2>,
    now: f64,
    geometry: Option<TooltipGeometry>,
    interaction: Option<TooltipInteractionState>,
) -> bool {
    let interaction = tooltip_interaction_for_geometry(geometry, interaction);
    if interaction.is_some_and(|state| state.dismissed) {
        return false;
    }
    if interaction.is_some_and(|state| state.focused || state.focus_requested) {
        return true;
    }
    geometry.is_some_and(|geometry| {
        geometry.handoff_until > now
            || geometry.pointer_inside_viewport
            || pointer.is_some_and(|pointer| {
                tooltip_region_contains(pointer, geometry.origin, geometry.card)
            })
    })
}

fn tooltip_interaction_for_geometry(
    geometry: Option<TooltipGeometry>,
    interaction: Option<TooltipInteractionState>,
) -> Option<TooltipInteractionState> {
    interaction.filter(|state| geometry.is_none_or(|geometry| geometry.identity == state.identity))
}

fn refresh_tooltip_root_geometry(
    mut geometry: TooltipGeometry,
    pointer: Option<Pos2>,
    now: f64,
) -> TooltipGeometry {
    // Pointer ownership transfers between native viewports. Once the cursor
    // enters the child, the root reports no pointer; only the child may clear
    // `pointer_inside_viewport`. The root owns route/deadline updates only.
    if pointer
        .is_some_and(|pointer| tooltip_region_contains(pointer, geometry.origin, geometry.card))
    {
        geometry.handoff_until = tooltip_handoff_deadline(now);
    }
    geometry
}

fn refresh_tooltip_child_geometry(
    mut geometry: TooltipGeometry,
    identity: u64,
    pointer_inside_viewport: bool,
) -> Option<TooltipGeometry> {
    // A native child can deliver its final pointer event after a competing
    // source has selected a new tooltip. Do not let that stale child mutate
    // either the replacement tooltip's geometry or its interaction state.
    if geometry.identity != identity {
        return None;
    }
    geometry.pointer_inside_viewport = pointer_inside_viewport;
    Some(geometry)
}

fn tooltip_handoff_deadline(now: f64) -> f64 {
    now + TOOLTIP_HANDOFF_GRACE.as_secs_f64()
}

fn tooltip_viewport_should_render(
    deterministic_scene: bool,
    root_focused: bool,
    handoff_active: bool,
    identity: u64,
    geometry: Option<TooltipGeometry>,
    interaction: Option<TooltipInteractionState>,
) -> bool {
    deterministic_scene
        || root_focused
        || handoff_active
        || geometry.is_some_and(|geometry| {
            geometry.identity == identity && geometry.pointer_inside_viewport
        })
        || interaction.is_some_and(|state| {
            state.identity == identity && (state.focused || state.focus_requested)
        })
}

fn trace_native_tooltip_handoff(
    pointer: Option<Pos2>,
    now: f64,
    geometry: Option<TooltipGeometry>,
    interaction: Option<TooltipInteractionState>,
    active: bool,
) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("TIPTOPTYP_UI_TRACE").is_some()) {
        return;
    }
    let pointer = pointer.map_or_else(|| "none".to_owned(), format_pos);
    let geometry = geometry.map_or_else(
        || "none".to_owned(),
        |geometry| {
            format!(
                "origin={} card={} viewport_inside={} until={:.3}",
                format_rect(geometry.origin),
                format_rect(geometry.card),
                geometry.pointer_inside_viewport,
                geometry.handoff_until,
            )
        },
    );
    let interaction = interaction.map_or_else(
        || "none".to_owned(),
        |interaction| {
            format!(
                "focused={} requested={} dismissed={}",
                interaction.focused, interaction.focus_requested, interaction.dismissed,
            )
        },
    );
    eprintln!(
        "ui.tooltip.handoff now={now:.3} pointer={pointer} {geometry} {interaction} active={active}"
    );
}

fn format_pos(pos: Pos2) -> String {
    format!("({:.1},{:.1})", pos.x, pos.y)
}

fn focus_tooltip_shortcut() -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Space)
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
        // Keep the currently visible native tooltip while the pointer crosses
        // another hoverable control on its way to that tooltip. Settings and
        // overrides use separate local cards and should retain their normal
        // independent behavior.
        let is_native_tooltip = id == native_hover_tooltip_id(&response.ctx);
        if !is_native_tooltip || !native_tooltip_handoff_blocks(&response.ctx, tooltip.origin) {
            response.ctx.data_mut(|data| data.insert_temp(id, tooltip));
        }
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
    link_sender: &mpsc::Sender<String>,
) {
    let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
        return;
    };
    let theme = context.theme();
    let style = context.style_of(theme);
    let tooltip_frame = theme::tooltip_card_frame(&style);
    let frame_margin = tooltip_frame.total_margin().sum();
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
    let width = (desired_card_width + frame_margin.x).min(available_width);
    let card_width = (width - frame_margin.x).max(1.0);
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
    let height = (METRICS.popup.tooltip_title_height + body_height + frame_margin.y)
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
    let interaction_id = tooltip_interaction_id(context);
    let identity = tooltip_identity(origin, detail);
    let interaction = context.data(|data| {
        data.get_temp::<TooltipInteractionState>(interaction_id)
            .filter(|state| state.identity == identity)
            .unwrap_or(TooltipInteractionState::new(identity))
    });
    if interaction.dismissed {
        return;
    }
    let geometry_id = tooltip_geometry_id(context);
    // The native child viewport is positioned in monitor coordinates, while
    // the root viewport reports pointer positions in its own local space. Keep
    // the card used by the pointer bridge in root-local coordinates; mixing
    // these spaces makes the bridge appear to work only at the top-left of the
    // screen.
    let root_local_card = root_local_tooltip_card(position, window_rect, Vec2::new(width, height));
    let now = context.input(|input| input.time);
    context.data_mut(|data| {
        let previous = data
            .get_temp::<TooltipGeometry>(geometry_id)
            .filter(|geometry| geometry.identity == identity);
        data.insert_temp(
            geometry_id,
            TooltipGeometry {
                identity,
                origin,
                card: root_local_card,
                // The child viewport exclusively owns this bit. The root has
                // no pointer while the cursor is over a native child and must
                // preserve the child's last observation across paint passes.
                pointer_inside_viewport: previous
                    .is_some_and(|geometry| geometry.pointer_inside_viewport),
                handoff_until: previous.map_or_else(
                    || tooltip_handoff_deadline(now),
                    |geometry| geometry.handoff_until,
                ),
            },
        );
    });
    let capture_viewport = captures.has_pending_for("diagnostic");
    let activate_viewport = capture_viewport || interaction.focus_requested || interaction.focused;

    // Keep the popup non-activating in production, while still letting it
    // receive pointer movement and wheel events for scrolling. A queued QA
    // capture temporarily activates its isolated viewport so macOS supplies
    // the repeated paint passes needed by the settling countdown.
    let spec = ChildViewSpec::tooltip(
        viewport_salt,
        "tiptoptyp",
        position,
        Vec2::new(width, height),
        activate_viewport,
        "diagnostic",
    );
    ChildViewHost::show(context, captures, spec, theme, &style, |ui, input| {
        let popup_focused = if capture_viewport {
            None
        } else {
            input.focused
        };
        let dismiss_requested = input.escape_pressed;
        ui.set_opacity(opacity);
        let frame = if interaction.focused {
            tooltip_frame.stroke(Stroke::new(
                1.0,
                style.visuals.widgets.active.bg_stroke.color,
            ))
        } else {
            tooltip_frame
        };
        let frame_response = frame.show(ui, |ui| {
            egui::ScrollArea::vertical()
                // Let short tooltips keep their natural height. Filling
                // the fixed native viewport makes the frame's content
                // rect reach the viewport edge, clipping its lower
                // rounded corners.
                .auto_shrink([false, true])
                .max_height(METRICS.popup.tooltip_max_height)
                .show(ui, |ui| show_markdown(ui, detail, link_sender));
        });
        let card_rect = frame_response.response.rect;
        let pointer_inside_viewport = ui.rect_contains_pointer(ui.max_rect());
        let pointer_inside_card = ui.rect_contains_pointer(card_rect);
        let popup_interacted = pointer_inside_card && ui.input(|input| input.pointer.any_pressed());
        let interaction = update_tooltip_interaction_state(
            interaction,
            identity,
            popup_interacted,
            popup_focused,
        );
        let interaction = if dismiss_requested {
            TooltipInteractionState {
                focused: false,
                focus_requested: false,
                dismissed: true,
                ..interaction
            }
        } else {
            interaction
        };
        context.data_mut(|data| {
            if let Some(geometry) = data.get_temp::<TooltipGeometry>(geometry_id)
                && let Some(geometry) =
                    refresh_tooltip_child_geometry(geometry, identity, pointer_inside_viewport)
            {
                // Lifetime follows the complete child viewport, including
                // transparent padding. Click/focus hit-testing above stays
                // restricted to the visibly painted card.
                data.insert_temp(geometry_id, geometry);
                data.insert_temp(interaction_id, interaction);
            }
        });
        if popup_interacted {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    });
}

fn show_local_tooltip_card(
    context: &egui::Context,
    anchor: Pos2,
    detail: &str,
    opacity: f32,
    link_sender: &mpsc::Sender<String>,
) {
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
                    .show(ui, |ui| show_markdown(ui, detail, link_sender));
            });
        });
}

fn frame_content_size(viewport_size: Vec2, frame_margin: Vec2) -> Vec2 {
    Vec2::new(
        (viewport_size.x - frame_margin.x).max(1.0),
        (viewport_size.y - frame_margin.y).max(1.0),
    )
}

fn show_markdown(ui: &mut egui::Ui, markdown: &str, link_sender: &mpsc::Sender<String>) {
    let mut fenced = false;
    let mut fence_token = String::new();
    let mut fence_lines = Vec::new();
    let highlighter = GenericSyntaxHighlighter::default();
    let mut typst_highlighter = SyntaxHighlighter::default();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if fenced {
                show_markdown_code_block(
                    ui,
                    &highlighter,
                    &mut typst_highlighter,
                    &fence_lines.join("\n"),
                    &fence_token,
                );
                fence_lines.clear();
                fence_token.clear();
            } else {
                fence_token = trimmed.trim_start_matches('`').trim().to_owned();
            }
            fenced = !fenced;
            continue;
        }
        if fenced {
            fence_lines.push(line.to_owned());
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
                ui.add(egui::Label::new(prefix).selectable(true));
            }
            show_markdown_inline(
                ui,
                content,
                heading,
                &highlighter,
                &mut typst_highlighter,
                link_sender,
            );
        });
    }
    if fenced {
        show_markdown_code_block(
            ui,
            &highlighter,
            &mut typst_highlighter,
            &fence_lines.join("\n"),
            &fence_token,
        );
    }
}

fn show_markdown_code_block(
    ui: &mut egui::Ui,
    highlighter: &GenericSyntaxHighlighter,
    typst_highlighter: &mut SyntaxHighlighter,
    source: &str,
    token: &str,
) {
    let dark_mode = ui.visuals().dark_mode;
    let token = token.trim().to_ascii_lowercase();
    let job = tooltip_code_job(highlighter, typst_highlighter, source, &token, dark_mode);
    if let Some(job) = job {
        ui.add(egui::Label::new(job).selectable(true).wrap());
    } else {
        let color = theme::syntax_palette(dark_mode).plain;
        ui.add(
            egui::Label::new(RichText::new(source).monospace().color(color))
                .selectable(true)
                .wrap(),
        );
    }
}

fn tooltip_code_job(
    highlighter: &GenericSyntaxHighlighter,
    typst_highlighter: &mut SyntaxHighlighter,
    source: &str,
    token: &str,
    dark_mode: bool,
) -> Option<egui::text::LayoutJob> {
    if matches!(token, "typ" | "typst" | "typc") {
        typst_highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::syntax_palette(dark_mode),
            None,
            &Default::default(),
        ));
        return Some(if token == "typc" {
            typst_highlighter.highlight_code(source, dark_mode, highlighter)
        } else {
            typst_highlighter.highlight(source, dark_mode, highlighter)
        });
    }
    highlighter.highlight_token(source, token, dark_mode)
}

fn show_markdown_inline(
    ui: &mut egui::Ui,
    text: &str,
    scale: f32,
    highlighter: &GenericSyntaxHighlighter,
    typst_highlighter: &mut SyntaxHighlighter,
    link_sender: &mpsc::Sender<String>,
) {
    let dark_mode = ui.visuals().dark_mode;
    for span in markdown_inline_spans(text) {
        if span.code {
            let job = tooltip_code_job(
                highlighter,
                typst_highlighter,
                &span.text,
                "typc",
                dark_mode,
            );
            if let Some(job) = job {
                ui.add(egui::Label::new(job).selectable(true).wrap());
                continue;
            }
        }
        let mut rich = RichText::new(span.text);
        if span.bold {
            rich = rich.strong();
        }
        if span.italics {
            rich = rich.italics();
        }
        if scale != 1.0 {
            rich = rich.size(theme::TYPE.content * scale);
        }
        if let Some(target) = span.link {
            let response = ui
                .add(
                    egui::Label::new(rich.color(ui.visuals().hyperlink_color).underline())
                        .selectable(true)
                        .sense(Sense::click_and_drag())
                        .wrap(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() && link_sender.send(target).is_ok() {
                ui.ctx().request_repaint();
            }
        } else {
            ui.add(egui::Label::new(rich).selectable(true).wrap());
        }
    }
}

fn markdown_inline_spans(text: &str) -> Vec<MarkdownInlineSpan> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut code = false;
    let mut bold = false;
    let mut italics = false;
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if code {
            if chars[index] == '`' {
                if !current.is_empty() {
                    push_markdown_span(&mut spans, &mut current, true, bold, italics, None);
                }
                code = false;
            } else {
                current.push(chars[index]);
            }
            index += 1;
            continue;
        }
        if chars[index] == '['
            && let Some((label, target, next_index)) = markdown_link_at(&chars, index)
        {
            push_markdown_span(&mut spans, &mut current, false, bold, italics, None);
            let link = normalize_browser_link_target(&target);
            spans.push(MarkdownInlineSpan {
                text: label,
                code: false,
                bold,
                italics,
                link,
            });
            index = next_index;
            continue;
        }
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
        let active = match kind {
            Some(1) => bold,
            Some(2) => italics,
            _ => false,
        };
        if let Some(kind) = kind
            && (active || marker_is_closed(&chars, index + marker_len, marker, marker_len))
        {
            push_markdown_span(&mut spans, &mut current, code, bold, italics, None);
            match kind {
                0 => code = !code,
                1 => bold = !bold,
                _ => italics = !italics,
            }
            index += marker_len;
        } else {
            current.extend(chars[index..index + marker_len.max(1)].iter().copied());
            index += marker_len.max(1);
        }
    }
    push_markdown_span(&mut spans, &mut current, code, bold, italics, None);
    spans
}

fn push_markdown_span(
    spans: &mut Vec<MarkdownInlineSpan>,
    current: &mut String,
    code: bool,
    bold: bool,
    italics: bool,
    link: Option<String>,
) {
    if !current.is_empty() {
        spans.push(MarkdownInlineSpan {
            text: std::mem::take(current),
            code,
            bold,
            italics,
            link,
        });
    }
}

fn markdown_link_at(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let label_end = find_unescaped_char(chars, start + 1, ']')?;
    if chars.get(label_end + 1) != Some(&'(') {
        return None;
    }
    let mut depth = 1_usize;
    let mut index = label_end + 2;
    while index < chars.len() {
        if chars[index] == '\\' {
            index = (index + 2).min(chars.len());
            continue;
        }
        match chars[index] {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let label = unescape_markdown(&chars[start + 1..label_end]);
                    let target = unescape_markdown(&chars[label_end + 2..index]);
                    if target.trim().is_empty() {
                        return None;
                    }
                    return Some((label, target.trim().to_owned(), index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn find_unescaped_char(chars: &[char], start: usize, needle: char) -> Option<usize> {
    let mut index = start;
    while index < chars.len() {
        if chars[index] == '\\' {
            index = (index + 2).min(chars.len());
            continue;
        }
        if chars[index] == needle {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn unescape_markdown(chars: &[char]) -> String {
    let mut output = String::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\\'
            && let Some(character) = chars.get(index + 1)
        {
            output.push(*character);
            index += 2;
        } else {
            output.push(chars[index]);
            index += 1;
        }
    }
    output
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

    // Use the straight corridor between the facing edges. The old triangular
    // bridge aimed at the card's center, which excluded perfectly natural
    // paths to the card's top or bottom edge and made the popup vanish during
    // the handoff.
    if card.left() >= origin.right() {
        let gap = card.left() - origin.right();
        if gap <= f32::EPSILON {
            return false;
        }
        let t = ((pointer.x - origin.right()) / gap).clamp(0.0, 1.0);
        let top = egui::lerp(origin.top()..=card.top(), t);
        let bottom = egui::lerp(origin.bottom()..=card.bottom(), t);
        pointer.y >= top && pointer.y <= bottom
    } else if card.right() <= origin.left() {
        let gap = origin.left() - card.right();
        if gap <= f32::EPSILON {
            return false;
        }
        let t = ((origin.left() - pointer.x) / gap).clamp(0.0, 1.0);
        let top = egui::lerp(card.top()..=origin.top(), t);
        let bottom = egui::lerp(card.bottom()..=origin.bottom(), t);
        pointer.y >= top && pointer.y <= bottom
    } else if card.top() >= origin.bottom() {
        let gap = card.top() - origin.bottom();
        if gap <= f32::EPSILON {
            return false;
        }
        let t = ((pointer.y - origin.bottom()) / gap).clamp(0.0, 1.0);
        let left = egui::lerp(origin.left()..=card.left(), t);
        let right = egui::lerp(origin.right()..=card.right(), t);
        pointer.x >= left && pointer.x <= right
    } else if card.bottom() <= origin.top() {
        let gap = origin.top() - card.bottom();
        if gap <= f32::EPSILON {
            return false;
        }
        let t = ((origin.top() - pointer.y) / gap).clamp(0.0, 1.0);
        let left = egui::lerp(card.left()..=origin.left(), t);
        let right = egui::lerp(card.right()..=origin.right(), t);
        pointer.x >= left && pointer.x <= right
    } else {
        false
    }
}

fn tooltip_identity(origin: Rect, detail: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    detail.hash(&mut hasher);
    for coordinate in [origin.min.x, origin.min.y, origin.max.x, origin.max.y] {
        coordinate.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

fn root_local_tooltip_card(position: Pos2, root_rect: Rect, size: Vec2) -> Rect {
    Rect::from_min_size(position - root_rect.min.to_vec2(), size)
}

fn update_tooltip_interaction_state(
    mut state: TooltipInteractionState,
    identity: u64,
    popup_interacted: bool,
    popup_focused: Option<bool>,
) -> TooltipInteractionState {
    if state.identity != identity {
        state = TooltipInteractionState::new(identity);
    }
    if popup_interacted {
        state.focus_requested = true;
    }
    match popup_focused {
        Some(true) => {
            state.focused = true;
            state.had_focus = true;
            state.dismissed = false;
        }
        Some(false) if state.had_focus => {
            state.focused = false;
            state.focus_requested = false;
            state.dismissed = true;
        }
        _ => {}
    }
    state
}

fn clamp_cursor_range(range: CCursorRange, len: usize) -> CCursorRange {
    CCursorRange {
        primary: CCursor::new(range.primary.index.0.min(len)),
        secondary: CCursor::new(range.secondary.index.0.min(len)),
        h_pos: range.h_pos,
    }
}

#[derive(Debug, Clone)]
struct CommentEdit {
    at: usize,
    remove: usize,
    insert: String,
}

fn toggle_line_comments(
    source: &str,
    selected: Range<usize>,
    prefix: &str,
) -> (String, Range<usize>) {
    let chars = source.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return (source.to_owned(), selected);
    }
    let len = chars.len();
    let start = selected.start.min(len);
    let end = selected.end.min(len).max(start);
    let line_start = chars[..start]
        .iter()
        .rposition(|character| *character == '\n')
        .map_or(0, |index| index + 1);
    let last_selected = end.saturating_sub(1).max(start);
    let line_end = chars[last_selected..]
        .iter()
        .position(|character| *character == '\n')
        .map_or(len, |offset| last_selected + offset);

    let mut line_starts = vec![line_start];
    for (index, character) in chars.iter().enumerate().take(line_end).skip(line_start) {
        if *character == '\n' {
            line_starts.push(index + 1);
        }
    }
    let mut edits = Vec::new();
    let mut commentable_lines = Vec::new();
    for line in line_starts {
        let mut content = line;
        while content < len && matches!(chars[content], ' ' | '\t') {
            content += 1;
        }
        let is_blank = content >= len || chars[content] == '\n';
        if !is_blank {
            commentable_lines.push((line, content));
        }
    }
    if commentable_lines.is_empty() {
        return (source.to_owned(), selected);
    }
    let uncomment = commentable_lines.iter().all(|(_, content)| {
        chars.get(*content) == Some(&'/') && chars.get(content + 1) == Some(&'/')
    });
    for (line, content) in commentable_lines {
        if uncomment {
            let remove = if chars.get(content + 2) == Some(&' ') {
                3
            } else {
                2
            };
            edits.push(CommentEdit {
                at: content,
                remove,
                insert: String::new(),
            });
        } else {
            edits.push(CommentEdit {
                at: content.max(line),
                remove: 0,
                insert: prefix.to_owned(),
            });
        }
    }
    let mut output = chars;
    for edit in edits.iter().rev() {
        let replacement = edit.insert.chars().collect::<Vec<_>>();
        output.splice(edit.at..edit.at + edit.remove, replacement);
    }
    let map = |index: usize| {
        let mut delta = 0isize;
        for edit in &edits {
            let insert_len = edit.insert.chars().count() as isize;
            let remove_len = edit.remove as isize;
            if index < edit.at {
                break;
            }
            if index <= edit.at + edit.remove {
                return (edit.at as isize + delta + insert_len).max(0) as usize;
            }
            delta += insert_len - remove_len;
        }
        (index as isize + delta).max(0) as usize
    };
    (output.into_iter().collect(), map(start)..map(end))
}

fn menu_button<'a>(
    ui: &egui::Ui,
    label: &'a str,
    shortcut: Option<KeyboardShortcut>,
) -> egui::Button<'a> {
    let button = egui::Button::new(label).frame(false);
    match shortcut {
        Some(shortcut) => button.shortcut_text(ui.ctx().format_shortcut(&shortcut)),
        None => button.right_text(""),
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

#[derive(Debug, Clone, Copy)]
struct CommandAvailability {
    can_undo: bool,
    can_redo: bool,
    typst_document: bool,
    interactive_preview: bool,
}

impl CommandAvailability {
    fn allows(self, requirement: CommandRequirement) -> bool {
        match requirement {
            CommandRequirement::Always => true,
            CommandRequirement::Undo => self.can_undo,
            CommandRequirement::Redo => self.can_redo,
            CommandRequirement::TypstDocument => self.typst_document,
            CommandRequirement::InteractivePreview => self.interactive_preview,
        }
    }
}

fn show_command_popup_ui(
    ui: &mut egui::Ui,
    menu: CommandMenu,
    availability: CommandAvailability,
    action: &mut Option<AppPopupAction>,
) {
    let mut previous_section = None;
    for spec in command_specs(menu).filter(|spec| spec.popup_section.is_some()) {
        if previous_section.is_some_and(|section| Some(section) != spec.popup_section) {
            ui.separator();
        }
        let enabled = availability.allows(spec.requirement);
        let shortcut = spec.shortcut.map(|shortcut| shortcut.egui());
        if menu_item_enabled(ui, enabled, spec.popup_title, shortcut).clicked() {
            *action = Some(AppPopupAction::Command(spec.command));
        }
        previous_section = spec.popup_section;
    }
}

fn show_file_popup_ui(ui: &mut egui::Ui, action: &mut Option<AppPopupAction>) {
    show_command_popup_ui(
        ui,
        CommandMenu::File,
        CommandAvailability {
            can_undo: false,
            can_redo: false,
            typst_document: true,
            interactive_preview: false,
        },
        action,
    );
}

fn show_edit_popup_ui(
    ui: &mut egui::Ui,
    can_undo: bool,
    can_redo: bool,
    can_format: bool,
    action: &mut Option<AppPopupAction>,
) {
    show_command_popup_ui(
        ui,
        CommandMenu::Edit,
        CommandAvailability {
            can_undo,
            can_redo,
            typst_document: can_format,
            interactive_preview: false,
        },
        action,
    );
}

fn show_view_popup_ui(ui: &mut egui::Ui, action: &mut Option<AppPopupAction>) {
    show_command_popup_ui(
        ui,
        CommandMenu::View,
        CommandAvailability {
            can_undo: false,
            can_redo: false,
            typst_document: false,
            interactive_preview: false,
        },
        action,
    );
}

fn show_workspace_popup_ui(
    ui: &mut egui::Ui,
    path: &Path,
    is_file: bool,
    preview_selected: bool,
    action: &mut Option<AppPopupAction>,
) {
    if menu_item_enabled(ui, is_file, "Open", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Open(
            path.to_path_buf(),
        )));
    }
    if menu_item_enabled(ui, is_file, "Open in New Window", None).clicked() {
        *action = Some(AppPopupAction::Workspace(
            WorkspaceMenuAction::OpenInNewWindow(path.to_path_buf()),
        ));
    }
    let is_typst = is_file
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("typ"));
    let preview_label = if preview_selected {
        "Stop using for preview"
    } else {
        "Use for preview"
    };
    if menu_item_enabled(ui, is_typst, preview_label, None).clicked() {
        *action = Some(AppPopupAction::Workspace(
            WorkspaceMenuAction::TogglePreview(path.to_path_buf()),
        ));
    }
    if menu_item_enabled(ui, is_file, "Rename…", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Rename(
            path.to_path_buf(),
        )));
    }
    ui.separator();
    if is_file {
        for kind in [
            WorkspaceCopyKind::FileName,
            WorkspaceCopyKind::FilePath,
            WorkspaceCopyKind::RelativePath,
        ] {
            if menu_item(ui, workspace_copy_kind_label(kind), None).clicked() {
                *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Copy {
                    path: path.to_path_buf(),
                    kind,
                }));
            }
        }
    } else if menu_item(ui, "Copy Path", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Copy {
            path: path.to_path_buf(),
            kind: WorkspaceCopyKind::FilePath,
        }));
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

fn workspace_snapshot_font_files(snapshot: &WorkspaceSnapshot) -> Vec<PathBuf> {
    fn collect(nodes: &[WorkspaceNode], files: &mut Vec<PathBuf>) {
        for node in nodes {
            if node.is_directory() && ignored_workspace_directory(&node.path) {
                continue;
            }
            if node.is_file() && is_font_path(&node.path) {
                files.push(node.path.clone());
            }
            collect(&node.children, files);
        }
    }

    let mut files = Vec::new();
    collect(&snapshot.nodes, &mut files);
    files.sort();
    files
}

fn popup_focus_should_close(
    had_focus: &mut bool,
    blur_started: &mut Option<Instant>,
    focused: Option<bool>,
    now: Instant,
) -> bool {
    match focused {
        Some(true) => {
            *had_focus = true;
            *blur_started = None;
            false
        }
        Some(false) if *had_focus => {
            let started = *blur_started.get_or_insert(now);
            now.saturating_duration_since(started) >= POPUP_BLUR_GRACE
        }
        Some(false) | None => false,
    }
}

fn app_popup_scroll_id(generation: u64) -> egui::Id {
    egui::Id::new(("app-popup-scroll", generation))
}

fn clamp_popup_above_anchor(anchor: Pos2, popup_size: Vec2, viewport_size: Vec2) -> Pos2 {
    clamp_popup_anchor(
        Pos2::new(anchor.x, anchor.y - popup_size.y),
        popup_size,
        viewport_size,
    )
}

fn status_log_popup_size(entry_count: usize) -> Vec2 {
    let content_height = 44.0 + entry_count.min(9) as f32 * STATUS_LOG_ROW_HEIGHT;
    Vec2::new(
        METRICS.menu.status_log_size.x,
        content_height.clamp(92.0, METRICS.menu.status_log_size.y),
    )
}

fn editor_context_menu_size(has_link: bool) -> Vec2 {
    let mut size = METRICS.menu.editor_size;
    if has_link {
        size.y += METRICS.menu.row_height + theme::SPACE.control;
    }
    size
}

fn workspace_context_menu_size(is_file: bool) -> Vec2 {
    let mut size = METRICS.menu.workspace_size;
    if is_file {
        // A file has three copy actions where a directory has one.
        size.y += METRICS.menu.row_height * 2.0;
    }
    size
}

const fn workspace_copy_kind_label(kind: WorkspaceCopyKind) -> &'static str {
    match kind {
        WorkspaceCopyKind::FileName => "Copy File Name",
        WorkspaceCopyKind::FilePath => "Copy File Path",
        WorkspaceCopyKind::RelativePath => "Copy Relative Path",
    }
}

const fn workspace_copy_kind_description(kind: WorkspaceCopyKind) -> &'static str {
    match kind {
        WorkspaceCopyKind::FileName => "file name",
        WorkspaceCopyKind::FilePath => "file path",
        WorkspaceCopyKind::RelativePath => "relative path",
    }
}

fn workspace_copy_text(path: &Path, root: &Path, kind: WorkspaceCopyKind) -> Option<String> {
    match kind {
        WorkspaceCopyKind::FileName => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        WorkspaceCopyKind::FilePath => Some(path.display().to_string()),
        WorkspaceCopyKind::RelativePath => path
            .strip_prefix(root)
            .ok()
            .map(|relative| relative.display().to_string()),
    }
}

fn child_viewport_builder(viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    #[cfg(not(target_os = "macos"))]
    {
        viewport.with_decorations(false)
    }
    #[cfg(target_os = "macos")]
    {
        viewport
    }
}

fn editor_web_link_at(data: &mut EditorDerivedData, char_index: usize) -> Option<String> {
    data.web_link_at(char_index)
        .and_then(|target| normalize_browser_link_target(&target))
}

fn editor_web_link_click_target(
    data: &mut EditorDerivedData,
    char_index: usize,
    clicked: bool,
    command: bool,
) -> Option<String> {
    (clicked && command)
        .then(|| editor_web_link_at(data, char_index))
        .flatten()
}

#[cfg(test)]
fn prepared_editor_data(source: &str) -> EditorDerivedData {
    let mut data = EditorDerivedData::default();
    data.prepare_source(EditorRevision::new(0, 0), source);
    data
}

#[cfg(test)]
fn typst_font_argument_at(source: &str, char_index: usize) -> Option<FontArgumentTarget> {
    prepared_editor_data(source).font_argument_at(char_index)
}

#[cfg(test)]
fn typst_web_link_at(source: &str, char_index: usize) -> Option<String> {
    editor_web_link_at(&mut prepared_editor_data(source), char_index)
}

#[cfg(test)]
fn typst_web_link_click_target(
    source: &str,
    char_index: usize,
    clicked: bool,
    command: bool,
) -> Option<String> {
    editor_web_link_click_target(
        &mut prepared_editor_data(source),
        char_index,
        clicked,
        command,
    )
}

fn normalize_browser_link_target(target: &str) -> Option<String> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    let target = if target
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("www."))
    {
        format!("https://{target}")
    } else {
        target.to_owned()
    };
    let url = url::Url::parse(&target).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn show_document_font_selector_ui(
    ui: &mut egui::Ui,
    catalog: &FontCatalog,
    target: &FontArgumentTarget,
    action: &mut Option<AppPopupAction>,
) {
    ui.label(RichText::new("Document font").strong());
    ui.label(
        RichText::new("Replace this text(font: …) value")
            .size(theme::TYPE.supporting)
            .weak(),
    );
    ui.separator();
    let families = catalog.document_family_names();
    if families.is_empty() {
        ui.label(RichText::new("Scanning installed and workspace fonts…").weak());
        return;
    }
    for family in families {
        if menu_item(ui, family, None).clicked() {
            *action = Some(AppPopupAction::SetDocumentFont {
                target: target.clone(),
                family: family.to_owned(),
            });
            ui.close();
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct EditorContextMenuOptions<'a> {
    can_undo: bool,
    can_redo: bool,
    has_selection: bool,
    can_format: bool,
    can_sync_preview: bool,
    link: Option<&'a str>,
}

fn show_editor_context_menu_ui(
    ui: &mut egui::Ui,
    options: EditorContextMenuOptions<'_>,
    action: &mut Option<EditorMenuAction>,
) {
    if let Some(link) = options.link {
        if menu_item(ui, "Open Link in Browser", None).clicked() {
            *action = Some(EditorMenuAction::OpenLink(link.to_owned()));
            ui.close();
        }
        ui.separator();
    }
    if menu_item_enabled(ui, options.can_undo, "Undo", None).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::Undo));
        ui.close();
    }
    if menu_item_enabled(ui, options.can_redo, "Redo", None).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::Redo));
        ui.close();
    }
    ui.separator();
    if menu_item_enabled(ui, options.has_selection, "Cut", None).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::Cut));
        ui.close();
    }
    if menu_item_enabled(ui, options.has_selection, "Copy", None).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::Copy));
        ui.close();
    }
    if menu_item(ui, "Paste", None).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::Paste));
        ui.close();
    }
    if menu_item(ui, "Select All", None).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::SelectAll));
        ui.close();
    }
    if menu_item(ui, "Toggle Comment", None).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::ToggleComment));
        ui.close();
    }
    if menu_item_enabled(ui, options.can_sync_preview, "Reveal in Preview", None).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::SyncPreview));
        ui.close();
    }
    if options.can_format {
        ui.separator();
        if menu_item(ui, "Format Document", None).clicked() {
            *action = Some(EditorMenuAction::Command(AppCommand::Format));
            ui.close();
        }
    }
}

fn push_status_log_entry(entries: &mut VecDeque<StatusLogEntry>, entry: StatusLogEntry) {
    entries.push_front(entry);
    entries.truncate(STATUS_LOG_LIMIT);
}

fn show_status_log_popup_ui(ui: &mut egui::Ui, entries: &VecDeque<StatusLogEntry>) {
    ui.label(RichText::new("Recent status").strong());
    ui.separator();
    if entries.is_empty() {
        ui.label(
            RichText::new("No status changes yet")
                .size(theme::TYPE.supporting)
                .weak(),
        );
        return;
    }
    for entry in entries {
        let color = notice_color(entry.kind, ui.visuals().dark_mode);
        ui.horizontal(|ui| {
            static_icon(
                ui,
                match entry.kind {
                    NoticeKind::Success => UiIcon::Check,
                    NoticeKind::Error => UiIcon::Warning,
                    NoticeKind::Info => UiIcon::Waiting,
                },
                color,
            );
            let detail_width =
                (ui.available_width() - STATUS_LOG_TIMESTAMP_WIDTH - ui.spacing().item_spacing.x)
                    .max(1.0);
            ui.add_sized(
                [detail_width, STATUS_LOG_ROW_HEIGHT],
                egui::Label::new(
                    RichText::new(&entry.detail)
                        .size(theme::TYPE.supporting)
                        .color(color),
                )
                .truncate(),
            )
            .on_hover_text(&entry.detail);
            ui.add_sized(
                [STATUS_LOG_TIMESTAMP_WIDTH, STATUS_LOG_ROW_HEIGHT],
                egui::Label::new(
                    RichText::new(&entry.timestamp)
                        .size(theme::TYPE.supporting)
                        .weak(),
                )
                .halign(Align::RIGHT),
            );
        });
    }
}

fn consume_preview_zoom_shortcut(input: &mut egui::InputState) -> Option<PreviewZoomAction> {
    let modifiers = Modifiers::COMMAND | Modifiers::ALT;
    for (key, action) in [
        (egui::Key::Plus, PreviewZoomAction::In),
        (egui::Key::Equals, PreviewZoomAction::In),
        (egui::Key::Minus, PreviewZoomAction::Out),
        (egui::Key::Num0, PreviewZoomAction::Reset),
    ] {
        if input.consume_shortcut(&KeyboardShortcut::new(modifiers, key)) {
            return Some(action);
        }
    }
    None
}

fn consume_ui_scale_shortcut(input: &mut egui::InputState) -> Option<i16> {
    for key in [egui::Key::Plus, egui::Key::Equals] {
        if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, key)) {
            return Some(5);
        }
    }
    input
        .consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Minus))
        .then_some(-5)
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

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    crate::private_workspace::AtomicFileWriter::write(path, contents)
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

fn preserve_workspace_snapshot_for_open(
    has_snapshot: bool,
    workspace_root: &Path,
    path: &Path,
) -> bool {
    has_snapshot && path.starts_with(workspace_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tinymist::LspPosition;

    fn run_shortcut<T>(
        modifiers: Modifiers,
        key: egui::Key,
        mut consume: impl FnMut(&mut egui::InputState) -> T,
    ) -> T {
        let context = egui::Context::default();
        let mut result = None;
        context
            .run_ui(
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
                    result = Some(ui.ctx().input_mut(&mut consume));
                },
            )
            .drop_without_applying_deltas();
        result.expect("shortcut resolver should run")
    }

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
    fn designated_typst_entry_remains_visible_while_editing_other_file_kinds() {
        for document_kind in [
            DocumentKind::Typst,
            DocumentKind::Text,
            DocumentKind::Pdf,
            DocumentKind::Image,
        ] {
            assert!(typst_preview_available_for(document_kind, true));
            assert!(preview_visible_for(document_kind, ViewMode::Split, true));
        }

        assert!(!typst_preview_available_for(DocumentKind::Text, false));
        assert!(!preview_visible_for(
            DocumentKind::Text,
            ViewMode::Split,
            false
        ));
        assert!(!preview_visible_for(
            DocumentKind::Text,
            ViewMode::Code,
            true
        ));
        assert!(preview_visible_for(
            DocumentKind::Pdf,
            ViewMode::Code,
            false
        ));
    }

    #[test]
    fn interactive_preview_does_not_duplicate_raster_compilation() {
        assert!(!raster_preview_required_for(true, false, false));
        assert!(raster_preview_required_for(true, true, false));
        assert!(raster_preview_required_for(true, false, true));
        assert!(raster_preview_required_for(false, false, false));
    }

    #[test]
    fn local_webview_failure_schedules_the_first_raster_fallback() {
        let raster_was_required = raster_preview_required_for(true, false, false);
        let raster_is_required = raster_preview_required_for(true, true, false);

        assert!(raster_fallback_compile_needed(
            true,
            true,
            raster_was_required,
            raster_is_required,
        ));
        assert!(!raster_fallback_compile_needed(
            false,
            true,
            raster_was_required,
            raster_is_required,
        ));
        assert!(!raster_fallback_compile_needed(
            true,
            false,
            raster_was_required,
            raster_is_required,
        ));
        assert!(!raster_fallback_compile_needed(true, true, true, true));
    }

    #[test]
    fn raster_results_require_the_matching_current_artifact() {
        let old = ArtifactKey {
            revision: 9,
            generation: 41,
        };
        let newer_same_revision = ArtifactKey {
            revision: 9,
            generation: 42,
        };
        assert!(raster_result_matches_artifact(
            newer_same_revision,
            9,
            Some(newer_same_revision)
        ));
        assert!(!raster_result_matches_artifact(
            old,
            9,
            Some(newer_same_revision)
        ));
        assert!(!raster_result_matches_artifact(old, 10, Some(old)));
        assert!(!raster_result_matches_artifact(old, 9, None));
    }

    #[test]
    fn same_revision_raster_from_an_older_artifact_is_stale_for_ui_actions() {
        let old = ArtifactKey {
            revision: 9,
            generation: 41,
        };
        let newer_same_revision = ArtifactKey {
            revision: 9,
            generation: 42,
        };

        assert_eq!(
            raster_content_freshness(true, Some(old), 9, Some(newer_same_revision)),
            Some(RasterContentFreshness::Stale)
        );
        assert_eq!(
            raster_content_freshness(
                true,
                Some(newer_same_revision),
                9,
                Some(newer_same_revision)
            ),
            Some(RasterContentFreshness::Current)
        );
        assert_eq!(
            raster_content_freshness(false, Some(old), 9, Some(newer_same_revision)),
            None
        );
    }

    #[test]
    fn egui_color_conversion_preserves_unmultiplied_alpha_channels() {
        // Use channels that survive Color32's quantized premultiplied storage exactly.
        let color = Color32::from_rgba_unmultiplied(255, 34, 68, 128);
        assert_eq!(rgba_from_color(color), Rgba::from_rgba(255, 34, 68, 128));
        assert_eq!(color_from_rgba(rgba_from_color(color)), color);
    }

    #[test]
    fn pausing_blocks_automatic_builds_but_not_explicit_pdf_or_capture_work() {
        assert!(compilation_run_allowed(false, false, false));
        assert!(!compilation_run_allowed(true, false, false));
        assert!(compilation_run_allowed(true, true, false));
        assert!(compilation_run_allowed(true, false, true));
        assert_eq!(
            compilation_toggle_copy(false),
            (
                "Pause",
                "Pause automatic preview updates; Compile PDF stays available"
            )
        );
        assert_eq!(
            compilation_toggle_copy(true),
            ("Resume", "Resume automatic preview updates")
        );
        assert_eq!(
            compilation_notice(true),
            ("Automatic preview updates paused", NoticeKind::Info)
        );
        assert_eq!(
            compilation_notice(false),
            ("Automatic preview updates resumed", NoticeKind::Success)
        );
    }

    #[test]
    fn compile_writes_beside_the_effective_saved_typst_entry() {
        assert_eq!(
            default_compile_pdf_path(
                None,
                DocumentKind::Typst,
                Some(Path::new("/project/chapters/main.typ")),
            ),
            Some(PathBuf::from("/project/chapters/main.pdf"))
        );
        assert_eq!(
            default_compile_pdf_path(
                Some(Path::new("/project/book.typ")),
                DocumentKind::Text,
                Some(Path::new("/project/metadata.toml")),
            ),
            Some(PathBuf::from("/project/book.pdf"))
        );
        assert_eq!(
            default_compile_pdf_path(None, DocumentKind::Typst, None),
            None,
            "an unsaved document must ask the user for an output path"
        );
        assert_eq!(
            default_compile_pdf_path(
                None,
                DocumentKind::Text,
                Some(Path::new("/project/notes.txt")),
            ),
            None
        );
    }

    #[test]
    fn compiling_snapshot_hides_pages_without_destroying_shared_raster_state() {
        assert!(snapshot_scene_hides_preview_pages(Some(
            UiSnapshotScene::PreviewCompiling
        )));
        assert!(!snapshot_scene_hides_preview_pages(Some(
            UiSnapshotScene::ProblemsPanel
        )));
        assert!(!snapshot_scene_hides_preview_pages(None));
    }

    #[test]
    fn raster_gated_snapshot_scenes_do_not_clobber_an_in_flight_build() {
        for scene in [
            UiSnapshotScene::Main,
            UiSnapshotScene::ProblemsPanel,
            UiSnapshotScene::FindReplace,
        ] {
            assert_eq!(settled_snapshot_preview_status(scene, false), None);
        }
        assert_eq!(
            settled_snapshot_preview_status(UiSnapshotScene::Main, true),
            Some(PreviewStatus::Ready(Duration::ZERO))
        );
        assert_eq!(
            settled_snapshot_preview_status(UiSnapshotScene::ProblemsPanel, true),
            Some(PreviewStatus::Error)
        );
    }

    #[test]
    fn pending_main_capture_queues_only_one_build_while_waiting_for_its_raster() {
        assert!(capture_preview_build_needed(
            true,
            false,
            false,
            PreviewStatus::Waiting,
            false,
        ));
        assert!(!capture_preview_build_needed(
            true,
            false,
            true,
            PreviewStatus::Waiting,
            false,
        ));
        assert!(!capture_preview_build_needed(
            true,
            false,
            false,
            PreviewStatus::Compiling,
            false,
        ));
        assert!(!capture_preview_build_needed(
            true,
            false,
            false,
            PreviewStatus::Ready(Duration::ZERO),
            true,
        ));
        assert!(!capture_preview_build_needed(
            true,
            false,
            false,
            PreviewStatus::Error,
            false,
        ));
    }

    #[test]
    fn pinned_restart_retains_exactly_one_existing_interactive_surface() {
        assert!(retain_preview_surface_for_restart(true, true, true, true));
        assert!(!retain_preview_surface_for_restart(false, true, true, true));
        assert!(!retain_preview_surface_for_restart(true, false, true, true));
        assert!(!retain_preview_surface_for_restart(true, true, false, true));
        assert!(!retain_preview_surface_for_restart(true, true, true, false));
    }

    #[test]
    fn replacement_preview_server_forces_navigation_even_when_its_url_is_reused() {
        let url = "http://127.0.0.1:4173/preview";
        assert!(!webview_navigation_required(Some(url), url, false));
        assert!(webview_navigation_required(Some(url), url, true));
        assert!(webview_navigation_required(
            Some(url),
            "http://127.0.0.1:4174/preview",
            false
        ));
    }

    #[test]
    fn command_clicking_a_link_takes_priority_over_source_preview_sync() {
        assert!(!source_preview_jump_gesture(
            SourcePreviewTrigger::ModifierClick,
            true,
            false,
            true,
            true,
        ));
        assert!(source_preview_jump_gesture(
            SourcePreviewTrigger::ModifierClick,
            true,
            false,
            true,
            false,
        ));
        assert!(source_preview_jump_gesture(
            SourcePreviewTrigger::DoubleClick,
            false,
            true,
            false,
            false,
        ));
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

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn reused_preview_webview_routes_against_the_current_server_origin() {
        let mut context = PreviewNavigationContext {
            base_url: "http://127.0.0.1:4173/preview".to_owned(),
            project_root: PathBuf::from("/tmp/project"),
            source_dir: Some(PathBuf::from("/tmp/project/chapters")),
        };
        let replacement = "http://127.0.0.1:4174/preview";
        assert_eq!(
            preview_navigation_action(&context, replacement),
            PreviewNavigationAction::Dispatch(replacement.to_owned())
        );

        context.base_url = replacement.to_owned();
        assert_eq!(
            preview_navigation_action(&context, replacement),
            PreviewNavigationAction::Embed
        );
        assert_eq!(
            preview_navigation_action(&context, "https://example.com/docs"),
            PreviewNavigationAction::Dispatch("https://example.com/docs".to_owned())
        );
        assert_eq!(
            preview_new_window_target(&context, "https://example.com/docs"),
            Some("https://example.com/docs".to_owned())
        );
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
            intent: PdfWriteIntent::Export,
        });
        assert_eq!(take_matching_export(&mut pending, 8), None);
        assert!(pending.is_none());

        let expected = PendingExport {
            path: PathBuf::from("current-document.pdf"),
            document_epoch: 8,
            intent: PdfWriteIntent::Compile,
        };
        let mut pending = Some(expected.clone());
        assert_eq!(take_matching_export(&mut pending, 8), Some(expected));
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
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"%PDF-exact\0bytes").unwrap();
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
    fn opening_a_file_only_invalidates_the_tree_when_the_workspace_changes() {
        let root = Path::new("/project");
        assert!(preserve_workspace_snapshot_for_open(
            true,
            root,
            Path::new("/project/chapters/intro.typ")
        ));
        assert!(!preserve_workspace_snapshot_for_open(
            true,
            root,
            Path::new("/another-project/main.typ")
        ));
        assert!(!preserve_workspace_snapshot_for_open(
            false,
            root,
            Path::new("/project/main.typ")
        ));
    }

    #[test]
    fn workspace_font_refresh_matches_catalog_directory_exclusions() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("assets/fonts")).unwrap();
        std::fs::create_dir_all(directory.path().join("node_modules/package")).unwrap();
        std::fs::write(directory.path().join("assets/fonts/local.otf"), b"font").unwrap();
        std::fs::write(
            directory.path().join("node_modules/package/ignored.ttf"),
            b"font",
        )
        .unwrap();
        std::fs::write(directory.path().join("notes.txt"), b"not a font").unwrap();

        let snapshot = WorkspaceSnapshot::scan(directory.path()).unwrap();
        let files = workspace_snapshot_font_files(&snapshot);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("assets/fonts/local.otf"));
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
    fn workspace_copy_actions_produce_name_absolute_and_root_relative_text() {
        let root = Path::new("/project");
        let path = root.join("docs/guide.typ");

        assert_eq!(
            workspace_copy_text(&path, root, WorkspaceCopyKind::FileName).as_deref(),
            Some("guide.typ")
        );
        assert_eq!(
            workspace_copy_text(&path, root, WorkspaceCopyKind::FilePath).as_deref(),
            Some("/project/docs/guide.typ")
        );
        assert_eq!(
            workspace_copy_text(&path, root, WorkspaceCopyKind::RelativePath).as_deref(),
            Some("docs/guide.typ")
        );
        assert_eq!(
            workspace_copy_text(
                Path::new("/elsewhere/guide.typ"),
                root,
                WorkspaceCopyKind::RelativePath,
            ),
            None
        );
    }

    #[test]
    fn file_context_menu_exposes_each_copy_contract() {
        use egui_kittest::{Harness, kittest::Queryable as _};

        let path = PathBuf::from("/project/docs/guide.typ");
        let expected_path = path.clone();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(320.0, 360.0))
            .build_ui_state(
                move |ui, action| {
                    show_workspace_popup_ui(ui, &path, true, false, action);
                },
                None::<AppPopupAction>,
            );
        harness.run();

        for label in ["Copy File Name", "Copy File Path", "Copy Relative Path"] {
            assert!(
                harness.query_by_label_contains(label).is_some(),
                "missing {label}"
            );
        }
        harness.get_by_label_contains("Copy Relative Path").click();
        harness.run();

        match harness.state() {
            Some(AppPopupAction::Workspace(WorkspaceMenuAction::Copy {
                path,
                kind: WorkspaceCopyKind::RelativePath,
            })) => assert_eq!(path, &expected_path),
            other => panic!("unexpected context-menu action: {other:?}"),
        }
        assert_eq!(
            workspace_context_menu_size(true).y,
            workspace_context_menu_size(false).y + METRICS.menu.row_height * 2.0
        );
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
    fn status_log_popup_hugs_its_bottom_anchor_and_only_grows_for_visible_rows() {
        let size = status_log_popup_size(4);
        assert_eq!(size.x, METRICS.menu.status_log_size.x);
        assert!(size.y < METRICS.menu.status_log_size.y);
        assert_eq!(status_log_popup_size(100).y, METRICS.menu.status_log_size.y);

        let anchor =
            clamp_popup_above_anchor(Pos2::new(500.0, 700.0), size, Vec2::new(1_000.0, 800.0));
        assert_eq!(anchor.y + size.y, 700.0);
    }

    #[test]
    fn popup_blur_requires_sustained_focus_loss_and_refocus_cancels_it() {
        let now = Instant::now();
        let mut had_focus = false;
        let mut blur_started = None;
        assert!(!popup_focus_should_close(
            &mut had_focus,
            &mut blur_started,
            Some(false),
            now,
        ));
        assert!(!popup_focus_should_close(
            &mut had_focus,
            &mut blur_started,
            Some(true),
            now,
        ));
        assert!(had_focus);
        assert!(!popup_focus_should_close(
            &mut had_focus,
            &mut blur_started,
            Some(false),
            now,
        ));
        assert!(!popup_focus_should_close(
            &mut had_focus,
            &mut blur_started,
            Some(true),
            now + POPUP_BLUR_GRACE / 2,
        ));
        assert_eq!(blur_started, None);
        assert!(!popup_focus_should_close(
            &mut had_focus,
            &mut blur_started,
            Some(false),
            now + POPUP_BLUR_GRACE,
        ));
        assert!(popup_focus_should_close(
            &mut had_focus,
            &mut blur_started,
            Some(false),
            now + POPUP_BLUR_GRACE * 2,
        ));
    }

    #[test]
    fn each_popup_open_gets_independent_scroll_memory() {
        assert_ne!(app_popup_scroll_id(1), app_popup_scroll_id(2));
    }

    #[test]
    fn settings_font_dropdown_opens_and_selects_a_catalog_family() {
        use egui_kittest::{Harness, kittest::Queryable as _};

        let catalog = FontCatalog::snapshot_fixture();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(520.0, 460.0))
            .build_ui_state(
                move |ui, selection| {
                    if let Some(next) = show_font_family_picker(
                        ui,
                        "tested-font-picker",
                        &catalog,
                        None,
                        None,
                        "System UI",
                        true,
                    ) {
                        *selection = Some(next);
                    }
                },
                None::<FontPickerSelection>,
            );
        harness.run();
        harness.get_by_value("System UI").click();
        harness.run();
        assert!(harness.query_by_label("Project Sans").is_some());
        harness.get_by_label("Project Sans").click();
        harness.run();
        assert!(matches!(
            harness.state(),
            Some(FontPickerSelection::Family { name, .. }) if name == "Project Sans"
        ));
    }

    #[test]
    fn tooltip_markdown_link_dispatches_its_normalized_target() {
        use egui_kittest::{Harness, kittest::Queryable as _};

        let (sender, receiver) = mpsc::channel();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(420.0, 160.0))
            .build_ui(move |ui| {
                show_markdown(ui, "Read [the guide](www\\.example.com/guide)", &sender);
            });
        harness.run();
        assert!(harness.query_by_label("www.example.com/guide").is_none());
        harness.get_by_label("the guide").click();
        harness.run();
        assert_eq!(
            receiver.try_recv(),
            Ok("https://www.example.com/guide".to_owned())
        );
    }

    #[test]
    fn editor_context_menu_exposes_the_link_under_the_pointer() {
        use egui_kittest::{Harness, kittest::Queryable as _};

        let mut harness = Harness::builder()
            .with_size(Vec2::new(320.0, 360.0))
            .build_ui_state(
                |ui, action| {
                    show_editor_context_menu_ui(
                        ui,
                        EditorContextMenuOptions {
                            can_undo: false,
                            can_redo: false,
                            has_selection: false,
                            can_format: true,
                            can_sync_preview: true,
                            link: Some("https://example.com/docs"),
                        },
                        action,
                    );
                },
                None::<EditorMenuAction>,
            );
        harness.run();
        harness
            .get_by_label_contains("Open Link in Browser")
            .click();
        harness.run();
        assert_eq!(
            harness.state(),
            &Some(EditorMenuAction::OpenLink(
                "https://example.com/docs".to_owned()
            ))
        );
        assert!(editor_context_menu_size(true).y > editor_context_menu_size(false).y);
    }

    #[test]
    fn typst_override_grid_centers_headers_labels_samples_and_actions() {
        use egui_kittest::{Harness, kittest::Queryable as _};

        let mut fonts_configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(1_600.0, 900.0))
            .build_ui(move |ui| {
                if !fonts_configured {
                    let _ = theme::configure_editor_fonts(
                        ui.ctx(),
                        theme::FontRequest::default(),
                        theme::FontRequest::default(),
                        false,
                        theme::FONT_WEIGHT_NORMAL,
                        theme::FONT_WEIGHT_NORMAL,
                    );
                    fonts_configured = true;
                    return;
                }
                show_typst_override_editor(
                    ui,
                    &mut TypstStyleOverrides::default(),
                    theme::syntax_palette(false),
                    &syntect::highlighting::Theme::default(),
                    &theme::FontWeightSupport::Discrete {
                        values: theme::EDITOR_FONT_WEIGHTS.to_vec(),
                        default: theme::FONT_WEIGHT_NORMAL,
                    },
                );
            });
        harness.run();

        let syntax = harness
            .get_all_by_label("Syntax")
            .next()
            .unwrap()
            .rect()
            .center()
            .y;
        let sample_header = harness
            .get_all_by_label("Live sample")
            .next()
            .unwrap()
            .rect()
            .center()
            .y;
        assert!((syntax - sample_header).abs() <= 0.5);

        let role = harness.get_by_label("Plain text").rect().center().y;
        let sample = harness.get_by_label("Document text").rect().center().y;
        let reset = harness
            .get_all_by_label("Reset")
            .next()
            .unwrap()
            .rect()
            .center()
            .y;
        assert!((role - sample).abs() <= 0.5);
        assert!((role - reset).abs() <= 0.5);
    }

    #[test]
    fn status_log_keeps_the_newest_one_hundred_entries() {
        let mut entries = VecDeque::new();
        for index in 0..105 {
            push_status_log_entry(
                &mut entries,
                StatusLogEntry {
                    timestamp: format!("{index}Z"),
                    detail: format!("entry {index}"),
                    kind: NoticeKind::Info,
                },
            );
        }
        assert_eq!(entries.len(), STATUS_LOG_LIMIT);
        assert_eq!(entries.front().unwrap().detail, "entry 104");
        assert_eq!(entries.back().unwrap().detail, "entry 5");
    }

    #[test]
    fn continuous_font_weight_is_committed_only_after_pointer_release() {
        let mut committed = 400;
        let mut staged = None;
        update_staged_font_weight(&mut committed, &mut staged, 535, true, true, false);
        assert_eq!(committed, 400);
        assert_eq!(staged, Some(535));

        update_staged_font_weight(&mut committed, &mut staged, 560, true, false, true);
        assert_eq!(committed, 560);
        assert_eq!(staged, None);

        update_staged_font_weight(&mut committed, &mut staged, 600, true, false, false);
        assert_eq!(committed, 600, "keyboard changes commit immediately");
    }

    #[test]
    fn font_argument_selector_targets_only_set_text_string_values() {
        let source = "= 文稿\n#set text(font: \"Inter\", size: 11pt)\n#text(font: \"Wrong\")[body]";
        let font_byte = source.find("font").unwrap();
        let font_char = source[..font_byte].chars().count();
        let target = typst_font_argument_at(source, font_char).unwrap();
        assert_eq!(&source[target.value_range], "Inter");

        let wrong_byte = source.rfind("font").unwrap();
        let wrong_char = source[..wrong_byte].chars().count();
        assert_eq!(typst_font_argument_at(source, wrong_char), None);
        assert_eq!(typst_font_argument_at("#set par(justify: true)", 8), None);
    }

    #[test]
    fn typst_web_links_are_detected_without_claiming_normal_editor_clicks() {
        let source = "= 文稿\n#link(\"www.example.com/docs?q=1\")[documentation]";
        for needle in ["link", "www.example.com", "documentation"] {
            let byte = source.find(needle).expect("fixture contains target");
            let character = source[..byte].chars().count();
            assert_eq!(
                typst_web_link_at(source, character),
                Some("https://www.example.com/docs?q=1".to_owned()),
                "pointer over {needle}"
            );
        }

        assert_eq!(typst_web_link_at(source, 0), None);
        let link_char = source[..source.find("link").unwrap()].chars().count();
        assert_eq!(
            typst_web_link_click_target(source, link_char, true, false),
            None,
            "an ordinary click must remain an editor action"
        );
        assert_eq!(
            typst_web_link_click_target(source, link_char, true, true),
            Some("https://www.example.com/docs?q=1".to_owned())
        );
        assert_eq!(typst_web_link_at("ordinary text", 0), None);
        assert_eq!(typst_web_link_at("#link(target)[dynamic]", 3), None);
        assert_eq!(typst_web_link_at("#link(\"file.typ\")[local]", 3), None);
        assert_eq!(
            typst_web_link_at("#other(\"https://example.com\")", 3),
            None
        );
    }

    #[test]
    fn browser_link_targets_accept_http_and_www_but_reject_unsafe_schemes() {
        assert_eq!(
            normalize_browser_link_target("www.example.com/path"),
            Some("https://www.example.com/path".to_owned())
        );
        assert_eq!(
            normalize_browser_link_target("https://example.com/path#part"),
            Some("https://example.com/path#part".to_owned())
        );
        assert_eq!(normalize_browser_link_target("javascript:alert(1)"), None);
        assert_eq!(normalize_browser_link_target("notes.typ"), None);
    }

    #[test]
    fn refresh_arrow_geometry_stays_outside_the_arc() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(14.0, 12.0));
        let geometry = refresh_icon_geometry(rect);
        let center = rect.center();
        let radius = geometry.arc[0].distance(center);

        assert_eq!(geometry.arc.last().copied(), Some(geometry.shaft[0]));
        assert_eq!(geometry.shaft[1], geometry.wing[0]);
        assert!(geometry.shaft[1].distance(center) > radius);

        let [a, b] = geometry.wing;
        let segment = b - a;
        let progress = ((center - a).dot(segment) / segment.length_sq()).clamp(0.0, 1.0);
        let closest = a + segment * progress;
        assert!(
            closest.distance(center) > radius,
            "the arrow wing must not cross back over the circular body"
        );
    }

    #[test]
    fn preview_eye_uses_symmetric_curves_instead_of_straight_facets() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(14.0));
        let geometry = eye_icon_geometry(rect);
        let center = rect.center();

        assert_eq!(geometry.upper[0], geometry.lower[0]);
        assert_eq!(geometry.upper[3], geometry.lower[3]);
        assert!(geometry.upper[1..3].iter().all(|point| point.y < center.y));
        assert!(geometry.lower[1..3].iter().all(|point| point.y > center.y));
        assert!(geometry.pupil_radius > METRICS.icon.stroke_width * 0.5);
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
    fn native_preview_bounds_stay_inside_the_panel_clip() {
        let available = Rect::from_min_max(Pos2::new(96.0, 30.0), Pos2::new(420.0, 260.0));
        let clip = Rect::from_min_max(Pos2::new(120.0, 48.0), Pos2::new(400.0, 220.0));

        assert_eq!(
            clipped_preview_rect(available, clip),
            Rect::from_min_max(Pos2::new(120.0, 48.0), Pos2::new(400.0, 220.0))
        );
    }

    #[test]
    fn native_preview_bounds_follow_egui_zoom_without_moving_the_viewport_origin() {
        assert_eq!(
            scale_rect_from_egui_to_native(
                Rect::from_min_max(Pos2::new(100.0, 50.0), Pos2::new(300.0, 250.0)),
                Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0)),
                1.15,
            ),
            Rect::from_min_max(Pos2::new(115.0, 57.5), Pos2::new(345.0, 287.5))
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
                    for ((id_salt, default_open), title) in
                        EXPLORER_SECTION_SPECS.into_iter().zip([
                            "Files",
                            "Contents",
                            "Subfiles",
                            "Symbols",
                            "Packages",
                            "Tags and references",
                        ])
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
        assert!(tooltip_region_contains(Pos2::new(20.0, 0.0), origin, card));
        assert!(tooltip_region_contains(Pos2::new(20.0, 10.0), origin, card));
        assert!(tooltip_region_contains(Pos2::new(45.0, 15.0), origin, card));
        assert!(!tooltip_region_contains(
            Pos2::new(20.0, 25.0),
            origin,
            card
        ));
    }

    #[test]
    fn tooltip_bridge_converts_child_position_to_root_coordinates() {
        let root = Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(800.0, 600.0));
        let card = root_local_tooltip_card(Pos2::new(420.0, 200.0), root, Vec2::new(240.0, 120.0));
        assert_eq!(card.min, Pos2::new(320.0, 150.0));
        assert!(card.contains(Pos2::new(400.0, 240.0)));
    }

    #[test]
    fn native_popup_content_budget_leaves_room_for_the_rendered_frame() {
        let style = egui::Style::default();
        let frame_margin = theme::tooltip_card_frame(&style).total_margin().sum();
        let content = frame_content_size(Vec2::new(200.0, 100.0), frame_margin);
        assert!(content.x + frame_margin.x <= 200.0);
        assert!(content.y + frame_margin.y <= 100.0);
        assert_eq!(
            frame_content_size(Vec2::splat(1.0), Vec2::splat(4.0)),
            Vec2::splat(1.0)
        );
    }

    #[test]
    fn tooltip_handoff_blocks_competing_hover_targets_until_focus_changes() {
        let geometry = TooltipGeometry {
            identity: 1,
            origin: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0)),
            card: Rect::from_min_max(Pos2::new(30.0, 0.0), Pos2::new(80.0, 30.0)),
            pointer_inside_viewport: false,
            handoff_until: 0.0,
        };
        assert!(tooltip_handoff_is_active(
            Some(Pos2::new(20.0, 0.0)),
            0.0,
            Some(geometry),
            None,
        ));
        assert!(!tooltip_handoff_blocks(
            true,
            Some(geometry.origin),
            geometry.origin,
        ));
        assert!(tooltip_handoff_blocks(
            true,
            Some(geometry.origin),
            Rect::from_min_max(Pos2::new(100.0, 0.0), Pos2::new(110.0, 10.0)),
        ));

        let mut focused = TooltipInteractionState::new(7);
        focused.focused = true;
        assert!(tooltip_handoff_is_active(
            Some(Pos2::new(500.0, 500.0)),
            0.0,
            None,
            Some(focused),
        ));
        assert!(!tooltip_handoff_blocks(
            true,
            None,
            Rect::from_min_max(Pos2::new(490.0, 490.0), Pos2::new(510.0, 510.0)),
        ));

        let mut dismissed = focused;
        dismissed.dismissed = true;
        assert!(!tooltip_handoff_is_active(
            Some(Pos2::new(500.0, 500.0)),
            0.0,
            Some(geometry),
            Some(dismissed),
        ));
    }

    #[test]
    fn tooltip_handoff_keeps_competing_targets_blocked_across_the_child_viewport() {
        let origin = Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(40.0, 30.0));
        let geometry = TooltipGeometry {
            identity: 2,
            origin,
            // The handoff envelope includes the transparent native viewport;
            // its painted card can be smaller after content-aware shrinking.
            card: Rect::from_min_max(Pos2::new(10.0, 50.0), Pos2::new(220.0, 140.0)),
            pointer_inside_viewport: false,
            handoff_until: 0.0,
        };
        let pointer = Pos2::new(100.0, 40.0);
        let active = tooltip_handoff_is_active(Some(pointer), 0.0, Some(geometry), None);
        assert!(active);
        assert!(tooltip_handoff_blocks(
            active,
            Some(origin),
            Rect::from_min_max(Pos2::new(90.0, 35.0), Pos2::new(120.0, 55.0)),
        ));
    }

    #[test]
    fn tooltip_handoff_grace_survives_a_transient_pointer_gap() {
        let geometry = TooltipGeometry {
            identity: 3,
            origin: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0)),
            card: Rect::from_min_max(Pos2::new(30.0, 0.0), Pos2::new(80.0, 30.0)),
            pointer_inside_viewport: false,
            handoff_until: 1.3,
        };
        let pointer = Some(Pos2::new(400.0, 400.0));
        assert!(tooltip_handoff_is_active(
            pointer,
            1.2,
            Some(geometry),
            None
        ));
        assert!(!tooltip_handoff_is_active(
            pointer,
            1.3,
            Some(geometry),
            None
        ));
    }

    #[test]
    fn tooltip_child_pointer_ownership_survives_missing_root_pointer_events() {
        let geometry = TooltipGeometry {
            identity: 4,
            origin: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0)),
            card: Rect::from_min_max(Pos2::new(30.0, 0.0), Pos2::new(80.0, 30.0)),
            pointer_inside_viewport: true,
            handoff_until: 0.5,
        };
        let refreshed = refresh_tooltip_root_geometry(geometry, None, 5.0);

        assert!(refreshed.pointer_inside_viewport);
        assert!(tooltip_handoff_is_active(None, 5.0, Some(refreshed), None));
        assert!(tooltip_viewport_should_render(
            false,
            false,
            false,
            geometry.identity,
            Some(refreshed),
            None,
        ));

        let replacement = TooltipGeometry {
            identity: 5,
            pointer_inside_viewport: false,
            ..geometry
        };
        assert!(refresh_tooltip_child_geometry(replacement, 4, true).is_none());
        let mut stale_focus = TooltipInteractionState::new(4);
        stale_focus.focused = true;
        assert!(!tooltip_handoff_is_active(
            None,
            5.0,
            Some(replacement),
            Some(stale_focus),
        ));
        assert!(tooltip_viewport_should_render(
            false,
            false,
            true,
            geometry.identity,
            None,
            None,
        ));
    }

    #[test]
    fn tooltip_click_requests_focus_and_defocus_dismisses_it() {
        let identity = 7;
        let state = TooltipInteractionState::new(identity);
        let state = update_tooltip_interaction_state(state, identity, true, Some(false));
        assert!(state.focus_requested);
        assert!(!state.dismissed);

        let state = update_tooltip_interaction_state(state, identity, false, Some(true));
        assert!(state.focused);
        assert!(state.had_focus);

        let state = update_tooltip_interaction_state(state, identity, false, Some(false));
        assert!(state.dismissed);
        assert!(!state.focus_requested);
    }

    #[test]
    fn tooltip_focus_shortcut_is_stable() {
        assert_eq!(
            focus_tooltip_shortcut(),
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Space)
        );
    }

    #[test]
    fn tooltip_typst_fences_respect_source_and_code_modes() {
        let generic = GenericSyntaxHighlighter::default();
        let mut typst = SyntaxHighlighter::default();
        let keyword = theme::syntax_palette(true).keyword;
        let color_at = |job: &egui::text::LayoutJob, byte| {
            job.sections
                .iter()
                .find(|section| {
                    section.byte_range.start.0 <= byte && byte < section.byte_range.end.0
                })
                .expect("highlighted byte should have a layout section")
                .format
                .color
        };

        let source = "#let value = 1";
        let source_job = tooltip_code_job(&generic, &mut typst, source, "typst", true).unwrap();
        assert_eq!(source_job.text, source);
        assert_eq!(color_at(&source_job, source.find("let").unwrap()), keyword);

        let code = "let value = 1";
        let code_job = tooltip_code_job(&generic, &mut typst, code, "typc", true).unwrap();
        assert_eq!(code_job.text, code);
        assert_eq!(color_at(&code_job, 0), keyword);
    }

    #[test]
    fn tooltip_inline_code_closes_without_leaking_its_style() {
        let span = |text: &str, code| MarkdownInlineSpan {
            text: text.to_owned(),
            code,
            bold: false,
            italics: false,
            link: None,
        };
        assert_eq!(
            markdown_inline_spans("The character `#` is invalid"),
            vec![
                span("The character ", false),
                span("#", true),
                span(" is invalid", false),
            ]
        );
        assert_eq!(
            markdown_inline_spans("An `unclosed marker"),
            vec![span("An `unclosed marker", false)]
        );
        assert_eq!(
            markdown_inline_spans("`stars * stay literal`"),
            vec![span("stars * stay literal", true)]
        );
    }

    #[test]
    fn tooltip_markdown_links_hide_destinations_and_preserve_code_literals() {
        assert_eq!(
            markdown_inline_spans("See [stuff](www\\.here.com/path) now"),
            vec![
                MarkdownInlineSpan {
                    text: "See ".to_owned(),
                    code: false,
                    bold: false,
                    italics: false,
                    link: None,
                },
                MarkdownInlineSpan {
                    text: "stuff".to_owned(),
                    code: false,
                    bold: false,
                    italics: false,
                    link: Some("https://www.here.com/path".to_owned()),
                },
                MarkdownInlineSpan {
                    text: " now".to_owned(),
                    code: false,
                    bold: false,
                    italics: false,
                    link: None,
                },
            ]
        );
        assert_eq!(
            markdown_inline_spans("`[literal](https://example.com)`"),
            vec![MarkdownInlineSpan {
                text: "[literal](https://example.com)".to_owned(),
                code: true,
                bold: false,
                italics: false,
                link: None,
            }]
        );
    }

    #[test]
    fn typst_override_toggle_hints_are_stateful_and_compact() {
        assert_eq!(typst_override_state(None), "inherit");
        assert_eq!(typst_override_state(Some(true)), "on");
        assert_eq!(typst_override_state(Some(false)), "off");
        assert_eq!(next_typst_override_state(None), Some(true));
        assert_eq!(next_typst_override_state(Some(true)), Some(false));
        assert_eq!(next_typst_override_state(Some(false)), None);

        let hint = typst_override_toggle_tooltip("B", None);
        assert_eq!(hint, "B: inherit; click to cycle");
        assert!(hint.chars().count() <= 32);
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
    fn ui_scale_is_clamped_to_the_supported_interface_range() {
        assert_eq!(ui_scale_factor(75), 0.75);
        assert_eq!(ui_scale_factor(DEFAULT_UI_SCALE_PERCENT), 1.0);
        assert_eq!(ui_scale_factor(150), 1.5);
        assert_eq!(ui_scale_factor(0), 0.75);
        assert_eq!(ui_scale_factor(u16::MAX), 1.5);
    }

    #[test]
    fn find_enter_navigation_honours_shift() {
        assert_eq!(find_step_for_enter(true, false), Some(FindStep::Next));
        assert_eq!(find_step_for_enter(true, true), Some(FindStep::Previous));
        assert_eq!(find_step_for_enter(false, true), None);
    }

    #[test]
    fn modified_shortcuts_win_over_their_generic_variants() {
        assert_eq!(
            run_shortcut(Modifiers::COMMAND | Modifiers::ALT, egui::Key::O, |input| {
                consume_shortcut(input, |command| {
                    command_spec(command).menu == CommandMenu::File
                })
            },),
            Some(AppCommand::OpenInNewWindow)
        );
        assert_eq!(
            run_shortcut(
                Modifiers::COMMAND | Modifiers::SHIFT,
                egui::Key::O,
                |input| consume_shortcut(input, |command| command_spec(command).menu
                    == CommandMenu::File),
            ),
            Some(AppCommand::ChangeWorkspaceRoot)
        );
        let replace = command_spec(AppCommand::FindReplace)
            .shortcut
            .expect("find/replace has a shortcut")
            .egui();
        assert_eq!(
            run_shortcut(replace.modifiers, replace.logical_key, |input| {
                consume_shortcut(input, |command| {
                    matches!(command, AppCommand::Find | AppCommand::FindReplace)
                })
            },),
            Some(AppCommand::FindReplace)
        );
    }

    #[test]
    fn preview_zoom_and_interface_scale_have_distinct_shortcuts() {
        let preview_modifiers = Modifiers::COMMAND | Modifiers::ALT;
        assert_eq!(
            run_shortcut(
                preview_modifiers,
                egui::Key::Plus,
                consume_preview_zoom_shortcut,
            ),
            Some(PreviewZoomAction::In)
        );
        assert_eq!(
            run_shortcut(
                Modifiers::COMMAND,
                egui::Key::Plus,
                consume_preview_zoom_shortcut,
            ),
            None
        );
        assert_eq!(
            run_shortcut(
                Modifiers::COMMAND,
                egui::Key::Plus,
                consume_ui_scale_shortcut,
            ),
            Some(5)
        );
    }

    #[test]
    fn workspace_colors_use_the_same_extension_policy_as_document_detection() {
        assert_eq!(
            workspace_entry_color(Path::new("main.typ"), false, false, false),
            workspace_entry_color(Path::new("main.TYP"), false, false, false)
        );
        assert_eq!(
            workspace_entry_color(Path::new("paper.pdf"), false, false, false),
            workspace_entry_color(Path::new("paper.PDF"), false, false, false)
        );
        assert_eq!(
            workspace_entry_color(Path::new("notes.jsonc"), false, false, false),
            theme::syntax_palette(false).plain
        );
        assert_eq!(
            workspace_entry_color(Path::new("favicon.ico"), false, false, false),
            theme::palette(false).info
        );
    }

    #[test]
    fn status_timestamps_are_explicitly_utc_and_wrap_at_midnight() {
        assert_eq!(utc_timestamp_from_unix_seconds(0), "00:00:00Z");
        assert_eq!(utc_timestamp_from_unix_seconds(3_661), "01:01:01Z");
        assert_eq!(utc_timestamp_from_unix_seconds(86_401), "00:00:01Z");
    }

    #[test]
    fn interface_scale_preserves_native_display_density() {
        for density in [1.0, 2.0] {
            let context = egui::Context::default();
            let mut input = egui::RawInput::default();
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(density);
            context
                .run_ui(input.clone(), |ui| apply_ui_scale(ui.ctx(), 125))
                .textures_delta
                .clear();
            context
                .run_ui(input, |ui| {
                    assert_eq!(ui.ctx().zoom_factor(), 1.25);
                    assert_eq!(ui.ctx().pixels_per_point(), density * 1.25);
                })
                .textures_delta
                .clear();
        }
    }

    #[test]
    fn toggle_line_comments_handles_selected_lines_and_round_trips() {
        let source = "  alpha\n\tbeta\n\n  gamma";
        let selected = 2..source.chars().count();
        let (commented, mapped) = toggle_line_comments(source, selected.clone(), "// ");
        assert_eq!(commented, "  // alpha\n\t// beta\n\n  // gamma");
        assert_eq!(mapped, 5..commented.chars().count());

        let (restored, restored_range) = toggle_line_comments(&commented, mapped, "// ");
        assert_eq!(restored, source);
        assert_eq!(restored_range, selected);
    }

    #[test]
    fn toggle_line_comments_uses_the_cursor_line_for_an_empty_selection() {
        let source = "one\ntwo\nthree";
        let (commented, cursor) = toggle_line_comments(source, 5..5, "// ");
        assert_eq!(commented, "one\n// two\nthree");
        assert_eq!(cursor, 8..8);
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
            let _ = theme::configure_editor_fonts(
                &context,
                theme::FontRequest::default(),
                theme::FontRequest::default(),
                false,
                theme::FONT_WEIGHT_NORMAL,
                theme::FONT_WEIGHT_NORMAL,
            );
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
