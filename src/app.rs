mod completion_popup;
mod navigation;
use navigation::EditorSelection;
mod explorer_view;
#[cfg(test)]
use explorer_view::{
    EXPLORER_SECTION_MIN_BODY_HEIGHT, explorer_section, explorer_section_body_height,
    explorer_section_state_id, workspace_asset_hover_rect, workspace_entry_color,
    workspace_entry_label, workspace_entry_resolved_color,
};
use explorer_view::{
    ExplorerSectionLayout, ExplorerSectionsSpec, add_workspace_nodes,
    available_explorer_section_body_height, explorer_section_layout_id,
    explorer_section_open_states, explorer_section_query_matches, git_command_opens_explorer,
    normalize_explorer_query, open_matching_workspace_ancestors, set_explorer_section_open,
    show_explorer_sections, show_project_index_section, workspace_node_matches_query,
    workspace_tree_state_id,
};
mod popup_layout;
#[cfg(test)]
use popup_layout::app_popup_scroll_id;
use popup_layout::{
    clamp_popup_above_anchor, clamp_popup_anchor, command_popup_size, editor_context_menu_size,
    show_popup_contents, status_log_popup_size, workspace_context_menu_size,
};
mod package_browser;
use package_browser::{PackageBrowserAction, PackageFilter, show_package_browser_ui};
mod editor_view;
mod extra_shortcuts;
mod git_actions;
mod lifecycle;
mod mitex_mode;
mod raster_view;
mod saves;
mod tabs;
mod workspace_view;
use lifecycle::DocumentLifecycle;
mod native_views;
mod settings_panel;
mod settings_view;
mod settings_window;
mod tooltips;
use settings_window::SettingsWindow;
use tooltips::*;
mod qa;
use qa::{QaSession, source_editor_snapshot_scroll_offset};
#[cfg(test)]
use qa::{STICKY_CONTEXT_SNAPSHOT_SOURCE, SceneDocument, prepare_sticky_context_snapshot_document};

use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tiptoptyp::save_transaction::{ExpectedDiskState, SaveInput, SaveIntent, fingerprint};
use tiptoptyp_core::geometry::{EguiRect, NativeRect, ViewportTransform};
#[cfg(test)]
use tiptoptyp_core::text::{LspPosition, LspRange};
use tiptoptyp_core::text::{LspTextEdit, ScalarOffset};
use tiptoptyp_core::text::{lsp_position_at_scalar, scalar_position_at};

use eframe::egui::{
    self, Align, Color32, ColorImage, KeyboardShortcut, Layout, Modifiers, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, TextureOptions, Vec2,
    text::{CCursor, CCursorRange},
};
use egui_ltreeview::{Action as TreeAction, TreeView, TreeViewState};
use rfd::AsyncFileDialog;

#[cfg(test)]
use crate::project_index::analyze_project;
use crate::{
    asset::{AssetLoader, AssetThumbnailLoader, LoadedAsset},
    builtin_themes,
    capabilities::CapabilityCache,
    child_view::{
        ChildViewHost, ChildViewSpec, POPUP_BLUR_GRACE, popup_focus_should_close,
        scoped_child_viewport_id, viewport_scoped_id,
    },
    compiler::{ArtifactKey, CompileEvent, CompileRequest, Compiler},
    diagnostics::{
        Diagnostic, DiagnosticLocation, DiagnosticSeverity, DiagnosticSource,
        normalize_diagnostics, parse_typst_short_output,
    },
    document::{DocumentKey, DocumentKind, DocumentSession, EditorSnapshot},
    editor_data::{EditorDerivedData, FontArgumentTarget, LineDiagnostic},
    editor_features::{
        EditableTable, PreviewAssetKind, SourceEdit, StickyContextQuery, StickyContextRow,
        editable_table_at,
    },
    explorer::{ExplorerOrder, ExplorerPanelState, ExplorerSection},
    font_catalog::{FontCatalog, FontFamily, ignored_workspace_directory, is_font_path},
    generic_highlight::GenericSyntaxHighlighter,
    highlight::SyntaxHighlighter,
    index_jobs::{Poll as ProjectIndexPoll, ProjectIndexClient, ProjectIndexInput},
    native_menu::{
        AppCommand, CommandMenu, CommandRequirement, NativeMenuCommandQueue, command_spec,
        command_specs, consume_shortcut,
    },
    package_catalog::{PackageCatalogLoad, PackageRoots},
    pdf::PreviewPage,
    pdf_pages::{PdfPageLoader, PdfSurface, RasterPageRequestKey},
    presentation::{
        ActiveThemeRequest, AppliedPresentation, ResolvedPresentationRequest,
        active_theme_preference, active_theme_request, load_active_theme_or_fallback,
        theme_request_for_appearance,
    },
    preview::{
        PAGE_GAP, PAGE_MARGIN, PDF_POINTS_PER_PREVIEW_PIXEL, PreviewController, PreviewEffect,
        PreviewStatus, PreviewStatusSnapshot, PreviewTexture, PreviewTransition,
        PreviewTransitionEvent, RasterContentFreshness, ResidentPreviewTexture, ServiceState,
        dark_preview_rgba, page_stack_geometry, stack_height, visible_page, visible_page_range,
        zoom_anchored_offset,
    },
    project_index::ProjectIndex,
    screenshot::{CaptureController, CaptureThemeProfile, UiCaptureStep, UiSnapshotScene},
    search::SearchSession,
    settings::{
        AppSettings, ColorThemeChoice, DEFAULT_UI_FONT_WEIGHT, DEFAULT_UI_SCALE_PERCENT,
        DocumentTheme, PreviewPreference, SourcePreviewTrigger, ToolMode, ToolPreference,
        normalize_workspace_root,
    },
    shortcuts::{
        ShortcutAction, ShortcutBindings, ShortcutChord, ShortcutPlatform, consume_shortcut_action,
    },
    sublime_theme::{self, ImportedTheme, Rgba},
    syntax_theme::{ResolvedTypstStyles, TypstStyleOverride, TypstStyleOverrides, TypstSyntaxRole},
    theme::{self, METRICS},
    tinymist::{
        CompletionItem, DiagnosticSeverity as TinymistDiagnosticSeverity, Generation, InvertColors,
        PreviewRefresh, TextDocument, TinymistConfig, TinymistDiagnostic, TinymistEvent,
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
    workspace_service::{WorkspaceClient, WorkspaceEvent},
};

#[cfg(test)]
use crate::editor_features::{StickyContextKind, sticky_context_rows};
#[cfg(test)]
use crate::presentation::{ThemeSourceRequest, load_active_theme};
#[cfg(test)]
use crate::preview::{
    preview_backend_label_for, preview_fallback_reason_for, raster_content_freshness,
    raster_result_matches_artifact,
};

const COMPILE_DEBOUNCE: Duration = Duration::from_millis(60);
const AUTOSAVE_RETRY_DELAY: Duration = Duration::from_secs(2);
const PROJECT_INDEX_DEBOUNCE: Duration = Duration::from_millis(180);
const MIN_PREVIEW_ZOOM: f32 = 0.2;
const MAX_PREVIEW_ZOOM: f32 = 6.0;
const STATUS_LOG_LIMIT: usize = 100;
const STATUS_LOG_TIMESTAMP_WIDTH: f32 = 74.0;
const STATUS_LOG_ROW_HEIGHT: f32 = 24.0;
const STICKY_CONTEXT_MAX_VIEWPORT_FRACTION: f32 = 0.45;
const STICKY_CONTEXT_STACK_RESOLUTION_LIMIT: usize = 32;
const STICKY_CONTEXT_BOTTOM_COVER: f32 = 1.0;
const STICKY_CONTEXT_SNAPSHOT_SCROLL_OFFSET: f32 = 176.0;

fn explorer_panel_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "filesystem")
}

fn explorer_width_restored_rect(rect: Rect, width: f32) -> Rect {
    Rect::from_min_size(rect.min, Vec2::new(width, rect.height()))
}

const ASSET_HOVER_CARD_MAX_IMAGE: Vec2 = Vec2::new(420.0, 300.0);
const ASSET_HOVER_LOADING_SIZE: Vec2 = Vec2::new(260.0, 112.0);
const ASSET_HOVER_ERROR_SIZE: Vec2 = Vec2::new(360.0, 144.0);
const WIDGET_PASTE_ADMISSION_FRAME_BUDGET: u64 = 8;

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

const fn close_request_requires_confirmation(
    close_requested: bool,
    dirty: bool,
    allow_close: bool,
    snapshot_scene: bool,
) -> bool {
    close_requested && dirty && !allow_close && !snapshot_scene
}

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

const fn view_mode_controls_enabled(document_kind: DocumentKind) -> bool {
    matches!(document_kind, DocumentKind::Typst)
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

const fn tinymist_preview_refresh(paused: bool) -> PreviewRefresh {
    if paused {
        // tiptoptyp never emits textDocument/didSave. Tinymist therefore keeps
        // serving the currently rendered document while its LSP still receives
        // every didChange needed by diagnostics, hover, formatting, and
        // completion.
        PreviewRefresh::OnSave
    } else {
        PreviewRefresh::OnType
    }
}

fn tinymist_language_features_ready(
    document_kind: DocumentKind,
    lsp_ready: bool,
    current_document_open: bool,
) -> bool {
    document_kind.is_typst() && lsp_ready && current_document_open
}

fn pdf_output_requires_new_artifact(
    typst_output: bool,
    automatic_preview_paused: bool,
    compile_scheduled: bool,
    status: PreviewStatus,
) -> bool {
    typst_output
        && (automatic_preview_paused
            || compile_scheduled
            || !matches!(status, PreviewStatus::Ready(_)))
}

fn pdf_artifact_reusable_for_output(
    artifact_key: Option<ArtifactKey>,
    has_pdf: bool,
    document_revision: u64,
    requires_new_artifact: bool,
) -> bool {
    !requires_new_artifact
        && has_pdf
        && artifact_key.is_some_and(|key| key.revision == document_revision)
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
        UiSnapshotScene::Main
        | UiSnapshotScene::Tabs
        | UiSnapshotScene::TabsPdf
        | UiSnapshotScene::TabsImage
        | UiSnapshotScene::FindReplace => Some(PreviewStatus::Ready(Duration::ZERO)),
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
    EyeClosed,
    Up,
    Warning,
    Waiting,
    ZoomIn,
    ZoomOut,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExternalFileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn external_file_stamp(path: &Path) -> std::io::Result<ExternalFileStamp> {
    let metadata = fs::metadata(path)?;
    Ok(ExternalFileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
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
    key: DocumentKey,
    range: Range<usize>,
    request_token: u64,
    uri: String,
    version: i32,
    requested: bool,
    detail: Option<Arc<str>>,
}

#[derive(Debug, Clone, Copy)]
enum TooltipRequest {
    Pointer(Pos2),
    Caret { key: DocumentKey, cursor: usize },
}

#[derive(Debug, Clone, Copy)]
struct EditorCaretState {
    key: DocumentKey,
    char_index: usize,
    rect: Rect,
}

#[derive(Debug, Clone)]
struct EditorCompletionState {
    key: DocumentKey,
    generation: Generation,
    uri: String,
    version: i32,
    request_token: u64,
    cursor: usize,
    // Cursor in `source`: canonical for LSP, displayed for local completions.
    source_cursor: usize,
    anchor: Rect,
    explicit: bool,
    is_incomplete: bool,
    selected: usize,
    items: Vec<CompletionItem>,
    all_items: Vec<CompletionItem>,
    source: String,
    local: bool,
}

#[derive(Debug, Clone)]
struct EditorCompletionResponse {
    generation: Generation,
    uri: String,
    version: i32,
    request_token: u64,
    is_incomplete: bool,
    items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardSemanticKind {
    Cut,
    Copy,
    Paste,
}

impl ClipboardSemanticKind {
    const fn key(self) -> egui::Key {
        match self {
            Self::Cut => egui::Key::X,
            Self::Copy => egui::Key::C,
            Self::Paste => egui::Key::V,
        }
    }

    const fn action(self) -> ShortcutAction {
        match self {
            Self::Cut => ShortcutAction::Cut,
            Self::Copy => ShortcutAction::Copy,
            Self::Paste => ShortcutAction::Paste,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingWidgetPaste {
    viewport: egui::ViewportId,
    expires_after_frame: u64,
}

impl PendingWidgetPaste {
    fn new(viewport: egui::ViewportId, requested_on_frame: u64) -> Self {
        Self {
            viewport,
            expires_after_frame: requested_on_frame
                .saturating_add(WIDGET_PASTE_ADMISSION_FRAME_BUDGET),
        }
    }

    fn admits(self, viewport: egui::ViewportId, frame: u64) -> bool {
        self.viewport == viewport && frame <= self.expires_after_frame
    }

    fn expired(self, frame: u64) -> bool {
        frame > self.expires_after_frame
    }
}

#[derive(Debug, Clone)]
struct DiagnosticTooltipOverlay {
    origin: Rect,
    anchor: Pos2,
    severity: DiagnosticSeverity,
    detail: String,
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
    Delete(PathBuf),
    Copy {
        path: PathBuf,
        kind: WorkspaceCopyKind,
    },
    Reveal(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RecentWorkspaceAction {
    Open(PathBuf),
    Remove(PathBuf),
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
    EditTable(EditableTable),
}

#[derive(Debug, Clone)]
struct TableEditorDialog {
    table: EditableTable,
    original_call: String,
    document_key: DocumentKey,
    focus_first_cell: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableEditorUiAction {
    Apply,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedTableSourceEdit {
    byte_range: Range<usize>,
    replacement: String,
    cursor: usize,
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
        // The active platform handle is only an exact match for a secondary
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
        table: Option<EditableTable>,
    },
    FontSelector {
        anchor: Pos2,
        target: FontArgumentTarget,
    },
    GitChunk {
        anchor: Pos2,
        chunk: crate::git::editor::ChunkDiff,
    },
    StatusLog {
        anchor: Pos2,
    },
}

#[derive(Debug, Clone)]
enum AppPopupAction {
    GitHunk(
        crate::git::repository::hunks::Action,
        DocumentKey,
        crate::git::editor::ChunkDiff,
    ),
    Command(AppCommand),
    Editor(EditorMenuAction),
    Workspace(WorkspaceMenuAction),
    SetDocumentFont {
        target: FontArgumentTarget,
        family: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    Appearance,
    Editor,
    Tools,
    Preview,
    Status,
}

impl SettingsSection {
    const fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Editor => "Editor",
            Self::Tools => "Tool binaries",
            Self::Preview => "Preview backend",
            Self::Status => "Toolchain status",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTarget {
    Appearance,
    TypstSyntax,
    LightTheme,
    DarkTheme,
    InvertColors,
    HueShift,
    ThemeColors,
    PageTheme,
    WrapLines,
    LineNumbers,
    StickyContextRows,
    AutoPairDelimiters,
    MitexDollars,
    RainbowBrackets,
    AutoSave,
    AutoSaveDelay,
    KeyboardShortcuts,
    ExplorerOrder,
    InterfaceScale,
    TitleBarMenus,
    FixedTabWidth,
    UiFont,
    UiFontWeight,
    CodeFont,
    CodeFontWeight,
    PreviewJump,
    HoverDelay,
    TypstCompiler,
    TinymistLanguageServer,
    RefreshBinaryStatus,
    BrowseTypstPackages,
    PreviewBackend,
    ToolchainStatus,
    ProjectRoot,
    UiScreenshots,
}

impl SettingsTarget {
    const ALL: [Self; 35] = [
        Self::Appearance,
        Self::TypstSyntax,
        Self::LightTheme,
        Self::DarkTheme,
        Self::InvertColors,
        Self::HueShift,
        Self::ThemeColors,
        Self::PageTheme,
        Self::WrapLines,
        Self::LineNumbers,
        Self::StickyContextRows,
        Self::AutoPairDelimiters,
        Self::MitexDollars,
        Self::RainbowBrackets,
        Self::AutoSave,
        Self::AutoSaveDelay,
        Self::KeyboardShortcuts,
        Self::ExplorerOrder,
        Self::InterfaceScale,
        Self::TitleBarMenus,
        Self::FixedTabWidth,
        Self::UiFont,
        Self::UiFontWeight,
        Self::CodeFont,
        Self::CodeFontWeight,
        Self::PreviewJump,
        Self::HoverDelay,
        Self::TypstCompiler,
        Self::TinymistLanguageServer,
        Self::RefreshBinaryStatus,
        Self::BrowseTypstPackages,
        Self::PreviewBackend,
        Self::ToolchainStatus,
        Self::ProjectRoot,
        Self::UiScreenshots,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::TypstSyntax => "Typst syntax",
            Self::LightTheme => "Light theme",
            Self::DarkTheme => "Dark theme",
            Self::InvertColors => "Invert colors",
            Self::HueShift => "Hue shift",
            Self::ThemeColors => "Color adjustments",
            Self::PageTheme => "Page",
            Self::WrapLines => "Wrap lines",
            Self::LineNumbers => "Line numbers",
            Self::StickyContextRows => "Sticky context rows",
            Self::AutoPairDelimiters => "Auto-close delimiters",
            Self::MitexDollars => "Auto-enable miTeX for compatible documents",
            Self::RainbowBrackets => "Rainbow brackets",
            Self::AutoSave => "Auto-save",
            Self::AutoSaveDelay => "Auto-save delay",
            Self::KeyboardShortcuts => "Keyboard shortcuts…",
            Self::ExplorerOrder => "Explorer panel order",
            Self::InterfaceScale => "Interface scale",
            Self::TitleBarMenus => "Show title-bar menus",
            Self::FixedTabWidth => "Fixed tab width",
            Self::UiFont => "UI font",
            Self::UiFontWeight => "UI font weight",
            Self::CodeFont => "Code font",
            Self::CodeFontWeight => "Code font weight",
            Self::PreviewJump => "Preview jump",
            Self::HoverDelay => "Hover delay",
            Self::TypstCompiler => "Typst compiler",
            Self::TinymistLanguageServer => "Tinymist language server",
            Self::RefreshBinaryStatus => "Refresh binary status",
            Self::BrowseTypstPackages => "Browse Typst packages…",
            Self::PreviewBackend => "Preview backend",
            Self::ToolchainStatus => "Toolchain status",
            Self::ProjectRoot => "Project root",
            Self::UiScreenshots => "UI screenshots",
        }
    }

    const fn section(self) -> SettingsSection {
        match self {
            Self::Appearance
            | Self::TypstSyntax
            | Self::LightTheme
            | Self::DarkTheme
            | Self::InvertColors
            | Self::HueShift
            | Self::ThemeColors
            | Self::PageTheme => SettingsSection::Appearance,
            Self::WrapLines
            | Self::LineNumbers
            | Self::StickyContextRows
            | Self::AutoPairDelimiters
            | Self::MitexDollars
            | Self::RainbowBrackets
            | Self::AutoSave
            | Self::AutoSaveDelay
            | Self::KeyboardShortcuts
            | Self::ExplorerOrder
            | Self::InterfaceScale
            | Self::TitleBarMenus
            | Self::FixedTabWidth
            | Self::UiFont
            | Self::UiFontWeight
            | Self::CodeFont
            | Self::CodeFontWeight
            | Self::PreviewJump
            | Self::HoverDelay => SettingsSection::Editor,
            Self::TypstCompiler
            | Self::TinymistLanguageServer
            | Self::RefreshBinaryStatus
            | Self::BrowseTypstPackages => SettingsSection::Tools,
            Self::PreviewBackend => SettingsSection::Preview,
            Self::ToolchainStatus | Self::ProjectRoot | Self::UiScreenshots => {
                SettingsSection::Status
            }
        }
    }

    const fn search_text(self) -> &'static str {
        match self {
            Self::Appearance => "interface system light dark theme mode active",
            Self::TypstSyntax => "colors colours overrides decorations theme",
            Self::LightTheme => "color colour palette import sublime builtin",
            Self::DarkTheme => "color colour palette import sublime builtin",
            Self::InvertColors => "theme transform inverse both themes",
            Self::HueShift => "theme transform degrees both themes color colour",
            Self::ThemeColors => {
                "theme colors colours invert inversion hue shift luminosity lightness brightness contrast saturation reset"
            }
            Self::PageTheme => "document light dark follow interface effective",
            Self::WrapLines => "editor soft wrapping",
            Self::LineNumbers => "editor gutter",
            Self::StickyContextRows => "editor headings scopes sections breadcrumbs",
            Self::AutoPairDelimiters => {
                "editor automatic pairing brackets quotes dollar backspace matching"
            }
            Self::MitexDollars => {
                "mitex latex tex dollar inline display block math package version translation"
            }
            Self::RainbowBrackets => {
                "editor color colour palettes cycles nesting parentheses square braces mixed math"
            }
            Self::AutoSave => "editor autosave automatic save",
            Self::AutoSaveDelay => "editor autosave automatic save milliseconds timing",
            Self::KeyboardShortcuts => "editor keys bindings configurable commands",
            Self::ExplorerOrder => {
                "explorer panels reorder files git contents subfiles symbols packages tags references"
            }
            Self::InterfaceScale => "editor ui zoom percent size",
            Self::TitleBarMenus => "editor titlebar file edit view chrome",
            Self::FixedTabWidth => "editor tabs documents fixed width size equal drag reorder",
            Self::UiFont => "editor interface family system choose",
            Self::UiFontWeight => "editor interface bold variable",
            Self::CodeFont => "editor source monospace family choose",
            Self::CodeFontWeight => "editor source monospace bold variable",
            Self::PreviewJump => "editor source sync click double modifier",
            Self::HoverDelay => "editor tooltip wait milliseconds timing",
            Self::TypstCompiler => "tools binary custom bundled path",
            Self::TinymistLanguageServer => "tools binary lsp custom bundled path",
            Self::RefreshBinaryStatus => "tools rescan reload",
            Self::BrowseTypstPackages => "tools package manager registry installed published",
            Self::PreviewBackend => "interactive raster pdf tinymist native retry",
            Self::ToolchainStatus => "tools typst tinymist lsp vector watcher pdf syntax ready",
            Self::ProjectRoot => "workspace folder directory path",
            Self::UiScreenshots => "capture png main settings both output directory",
        }
    }

    fn matches_terms(self, terms: &[String]) -> bool {
        let haystack = format!(
            "{} {} {}",
            self.label(),
            self.section().title(),
            self.search_text()
        )
        .to_lowercase();
        terms.iter().all(|term| haystack.contains(term))
    }
}

fn settings_search_results(query: &str) -> Vec<SettingsTarget> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    SettingsTarget::ALL
        .into_iter()
        .filter(|target| target.matches_terms(&terms))
        .collect()
}

fn take_settings_scroll_target(
    pending: &mut Option<SettingsTarget>,
    rendered: SettingsTarget,
) -> bool {
    if *pending != Some(rendered) {
        return false;
    }
    *pending = None;
    true
}

pub struct EditorApp {
    tabs: tabs::Tabs,
    lifecycle: DocumentLifecycle,
    process_close_pending: bool,
    highlighter: SyntaxHighlighter,
    auto_pair_syntax: crate::auto_pairs::PairSyntax,
    generic_highlighter: GenericSyntaxHighlighter,

    compiler: Compiler,
    asset_loader: AssetLoader,
    preview_page_loader: PdfPageLoader,
    asset_page_loader: PdfPageLoader,
    asset_preview: PreviewController,
    asset_token: crate::asset::AssetToken,
    asset_thumbnail_loader: AssetThumbnailLoader,
    asset_thumbnail_token: crate::asset::ThumbnailToken,
    asset_hover: Option<AssetHoverState>,
    pending_asset_page: Option<usize>,
    compile_deadline: Option<Instant>,
    compilation_paused: bool,
    preview: PreviewController,
    editor_data: EditorDerivedData,

    view_mode: ViewMode,
    explorer: ExplorerPanelState,
    focus_explorer_search: bool,
    problems_visible: bool,
    settings_visible: bool,
    settings_open_requested: bool,
    settings_window: Arc<std::sync::Mutex<SettingsWindow>>,
    retain_settings_viewport: bool,
    shortcut_editor_visible: bool,
    shortcut_query: String,
    shortcut_capture: Option<ShortcutAction>,
    shortcut_notice: Option<String>,
    pending_widget_paste: Option<PendingWidgetPaste>,
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
    packages_visible: bool,
    package_query: String,
    package_filter: PackageFilter,
    package_catalog: Option<PackageCatalogLoad>,
    package_catalog_job: LatestJob<PackageCatalogLoad>,
    tool_refresh_requested: bool,
    typst_tool: ToolResolution,
    tinymist_tool: ToolResolution,
    capabilities: CapabilityCache,
    workspace_root: PathBuf,
    git: crate::git::GitPanel,
    // The default Git section should be visible even if an older session
    // persisted it as collapsed. Opening Git from the command menu uses the
    // same one-shot reveal, preserving subsequent user choice.
    git_explorer_reveal: bool,
    git_editor: crate::git::editor::GitEditorState,
    git_hunk_job: crate::worker::ExclusiveJob<String>,
    workspace_chooser_visible: bool,
    workspace_history_removals: Vec<PathBuf>,
    workspace: Option<WorkspaceTree>,
    workspace_error: Option<String>,
    file_import: crate::worker::ExclusiveJob<String>,
    package_uninstall: crate::worker::ExclusiveJob<String>,
    workspace_service: WorkspaceClient,
    project_index: ProjectIndex,
    project_index_deadline: tiptoptyp_core::scheduling::Debounce<Instant>,
    project_index_job: ProjectIndexClient,
    captures: CaptureController,
    snapshot_scene: Option<UiSnapshotScene>,
    qa: QaSession,
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
    pending_editor_selection: Option<EditorSelection>,
    editor_attention: Option<EditorAttention>,
    editor_hover: Option<EditorHoverState>,
    next_editor_hover_token: u64,
    tooltip_request: Option<TooltipRequest>,
    editor_completion: Option<EditorCompletionState>,
    next_editor_completion_token: u64,
    last_editor_caret: Option<EditorCaretState>,
    manual_format_revision: Option<u64>,
    format_request_key: Option<DocumentKey>,
    format_when_tinymist_ready: Option<DocumentKey>,
    diagnostic_tooltip: Option<DiagnosticTooltipOverlay>,
    app_popup: Option<AppPopup>,
    app_popup_generation: u64,
    app_popup_had_focus: bool,
    app_popup_blur_started: Option<Instant>,
    pending_app_popup_action: Option<AppPopupAction>,
    document_workflow: DocumentWorkflow,
    save_job: crate::worker::ExclusiveJob<crate::save_io::SaveResult>,
    pending_save: Option<saves::PendingSave>,
    rename_dialog: Option<RenameDialog>,
    rename_overlay_had_focus: bool,
    rename_overlay_suspended: bool,
    table_editor: Option<TableEditorDialog>,
    table_editor_had_focus: bool,
    table_editor_suspended: bool,

    pending_tool_picker: Option<PendingDialog<ToolPickerTarget>>,
    notice: Option<Notice>,
    status_log: VecDeque<StatusLogEntry>,
    recorded_status: Option<PreviewStatus>,
    recorded_notice: Option<Notice>,
    external_file_stamp: Option<ExternalFileStamp>,
    external_file_change_notice: Option<ExternalFileObservation>,
    last_title: String,

    tinymist: TinymistSidecar,
    tinymist_sync: crate::tinymist_sync::Coordinator,

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
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview_applied: Option<native_views::WebviewAppliedState>,
    web_link_sender: mpsc::Sender<String>,
    web_link_receiver: mpsc::Receiver<String>,
    browser_launch: Option<mpsc::Receiver<Result<String, String>>>,
    browser_repaint: crate::worker::RepaintTarget,
}

impl EditorApp {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        captures: CaptureController,
        theme_override: Option<CaptureThemeProfile>,
        snapshot_scene: Option<UiSnapshotScene>,
    ) -> Self {
        let (settings, load_error) = match AppSettings::load(context.storage) {
            Ok(settings) => (settings, None),
            Err(error) => (AppSettings::default(), Some(error.to_string())),
        };
        let mut app = Self::new_session(
            &context.egui_ctx,
            egui::ViewportId::ROOT,
            initial_path,
            captures,
            theme_override,
            snapshot_scene,
            settings,
            EditorWindowHost::Root,
            DocumentLifecycle::Active,
        );
        if let Some(message) = load_error {
            push_status_log_entry(
                &mut app.status_log,
                StatusLogEntry {
                    timestamp: current_timestamp(),
                    detail: message.clone(),
                    kind: NoticeKind::Error,
                },
            );
            app.notice = Some(Notice {
                message,
                kind: NoticeKind::Error,
            });
            app.recorded_notice = app.notice.clone();
        }
        app
    }

    #[allow(clippy::too_many_arguments)]
    fn new_session(
        context: &egui::Context,
        viewport: egui::ViewportId,
        initial_path: Option<PathBuf>,
        captures: CaptureController,
        theme_override: Option<CaptureThemeProfile>,
        snapshot_scene: Option<UiSnapshotScene>,
        mut settings: AppSettings,
        window_host: EditorWindowHost,
        lifecycle: DocumentLifecycle,
    ) -> Self {
        if snapshot_scene.is_some() {
            settings.ui_font_weight = DEFAULT_UI_FONT_WEIGHT;
            settings.code_font_weight = DEFAULT_UI_FONT_WEIGHT;
            settings.explorer_order = ExplorerOrder::default();
            settings.rainbow_brackets = crate::rainbow::RainbowBrackets::default();
        }
        let invalid_initial_path = initial_path
            .as_ref()
            .filter(|path| !path.is_file() && !path.is_dir())
            .cloned();
        let working_directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let initial_workspace =
            resolve_initial_workspace(&settings, initial_path.as_deref(), &working_directory);
        let open_empty_workspace =
            snapshot_scene.is_none() && initial_path.as_ref().is_some_and(|path| path.is_dir());
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
        theme::set_imported_palette(
            context,
            Some((active_theme.dark_mode, active_theme.palette)),
        );
        let typst_tool = resolve_tool(ToolKind::Typst, &settings.typst);
        let tinymist_tool = resolve_tool(ToolKind::Tinymist, &settings.tinymist);
        let capabilities = CapabilityCache::discover();
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
            theme::syntax_palette_from_semantic(active_theme.palette),
            Some(&active_theme.syntect_theme),
            settings.typst_overrides.for_dark(active_theme.dark_mode),
        ));
        let owner = tiptoptyp_core::document::WindowSessionId::new(viewport.0.value());
        let mut app = Self {
            tabs: tabs::Tabs::new(
                snapshot_scene.is_none(),
                DocumentSession::new(owner, DEFAULT_SOURCE, DocumentKind::Typst),
                workspace_root.clone(),
            ),
            lifecycle,
            process_close_pending: false,
            highlighter,
            auto_pair_syntax: crate::auto_pairs::PairSyntax::default(),
            generic_highlighter,
            compiler: Compiler::new(crate::worker::RepaintTarget::new(context, viewport)),
            asset_loader: AssetLoader::new(crate::worker::RepaintTarget::new(context, viewport)),
            preview_page_loader: PdfPageLoader::new(crate::worker::RepaintTarget::new(
                context, viewport,
            )),
            asset_page_loader: PdfPageLoader::new(crate::worker::RepaintTarget::new(
                context, viewport,
            )),
            asset_preview: PreviewController::new(false, PreviewPreference::Native),
            asset_token: Default::default(),
            asset_thumbnail_loader: AssetThumbnailLoader::new(crate::worker::RepaintTarget::new(
                context, viewport,
            )),
            asset_thumbnail_token: Default::default(),
            asset_hover: None,
            pending_asset_page: None,
            compile_deadline: lifecycle.allows_document_work().then(Instant::now),
            compilation_paused: false,
            preview: PreviewController::new(preview_dark, settings.preview_preference),
            editor_data: EditorDerivedData::default(),
            view_mode: ViewMode::Split,
            explorer: ExplorerPanelState::default(),
            problems_visible: false,
            settings_visible: false,
            settings_open_requested: false,
            settings_window: Arc::default(),
            retain_settings_viewport: false,
            shortcut_editor_visible: false,
            focus_explorer_search: false,
            shortcut_query: String::new(),
            shortcut_capture: None,
            shortcut_notice: None,
            pending_widget_paste: None,
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
            packages_visible: false,
            package_query: String::new(),
            package_filter: PackageFilter::All,
            package_catalog: None,
            package_catalog_job: LatestJob::default(),
            tool_refresh_requested: false,
            typst_tool,
            tinymist_tool,
            capabilities,
            workspace_root,
            git: crate::git::GitPanel::default(),
            git_explorer_reveal: true,
            git_editor: crate::git::editor::GitEditorState::default(),
            git_hunk_job: Default::default(),
            workspace_chooser_visible: false,
            workspace_history_removals: Vec::new(),
            workspace: None,
            workspace_error: None,
            file_import: Default::default(),
            package_uninstall: Default::default(),
            workspace_service: WorkspaceClient::new(
                owner,
                crate::worker::RepaintTarget::new(context, viewport),
            ),
            project_index: ProjectIndex::default(),
            project_index_deadline: if lifecycle.allows_document_work() {
                tiptoptyp_core::scheduling::Debounce::at(Instant::now())
            } else {
                Default::default()
            },
            project_index_job: ProjectIndexClient::new(owner),
            captures,
            snapshot_scene,
            qa: QaSession::default(),
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
            tooltip_request: None,
            editor_completion: None,
            next_editor_completion_token: 1,
            last_editor_caret: None,
            manual_format_revision: None,
            format_request_key: None,
            format_when_tinymist_ready: None,
            diagnostic_tooltip: None,
            app_popup: None,
            app_popup_generation: 0,
            app_popup_had_focus: false,
            app_popup_blur_started: None,
            pending_app_popup_action: None,
            document_workflow: DocumentWorkflow::default(),
            save_job: Default::default(),
            pending_save: None,
            rename_dialog: None,
            rename_overlay_had_focus: false,
            rename_overlay_suspended: false,
            table_editor: None,
            table_editor_had_focus: false,
            table_editor_suspended: false,
            pending_tool_picker: None,
            notice: None,
            status_log: VecDeque::new(),
            recorded_status: None,
            recorded_notice: None,
            external_file_stamp: None,
            external_file_change_notice: None,
            last_title: String::new(),
            tinymist: TinymistSidecar::new(crate::worker::RepaintTarget::new(context, viewport)),
            tinymist_sync: Default::default(),
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
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview_applied: None,
            web_link_sender,
            web_link_receiver,
            browser_launch: None,
            browser_repaint: crate::worker::RepaintTarget::new(context, viewport),
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

        if open_empty_workspace {
            app.empty_workspace(context);
            app.refresh_workspace();
        } else if let Some(path) = initial_document {
            if !app.load_path(path) {
                app.reset_document_services();
                app.schedule_compile_now();
            }
        } else {
            if app.settings.mitex_auto_enable && app.snapshot_scene.is_none() {
                app.document_mut().replace_unprojected_untitled("");
                app.activate_preferred_tex();
            }
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
        viewport: egui::ViewportId,
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
            viewport,
            initial_path,
            captures,
            None,
            None,
            settings,
            EditorWindowHost::Secondary,
            DocumentLifecycle::Active,
        );
        if let Some(workspace_root) = untitled_workspace {
            app.workspace_root = canonical_or_absolute(&workspace_root);
            app.remember_workspace(&app.workspace_root.clone());
            app.new_tab(context);
            app.refresh_workspace();
        }
        app
    }

    #[cfg(test)]
    pub(crate) fn dormant_for_tests(context: &egui::Context, root: PathBuf) -> Self {
        Self::dormant_window_for_tests(context, root, egui::ViewportId::ROOT)
    }

    #[cfg(test)]
    pub(crate) fn dormant_window_for_tests(
        context: &egui::Context,
        root: PathBuf,
        viewport: egui::ViewportId,
    ) -> Self {
        Self::new_session(
            context,
            viewport,
            Some(root),
            CaptureController::disabled_for_tests(),
            None,
            Some(UiSnapshotScene::Main),
            AppSettings::default(),
            if viewport == egui::ViewportId::ROOT {
                EditorWindowHost::Root
            } else {
                EditorWindowHost::Secondary
            },
            DocumentLifecycle::Dormant,
        )
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

    pub(crate) fn document_path_for_profile(&self) -> PathBuf {
        self.document()
            .path()
            .clone()
            .expect("profiling uses a saved fixture")
    }

    pub(crate) fn native_command_enabled(&self, command: AppCommand) -> bool {
        if self.tabs.is_empty() {
            return workspace_view::empty_workspace_command(command);
        }
        match command_spec(command).requirement {
            CommandRequirement::Always => true,
            CommandRequirement::SavedDocument => self.document().path().is_some(),
            // Undo/redo history is viewport-local egui state. Keep these menu
            // items enabled so the active viewport can make the final choice.
            CommandRequirement::Undo | CommandRequirement::Redo => true,
            CommandRequirement::TypstDocument => self.document().kind().is_typst(),
            CommandRequirement::TypstPreview => self.typst_preview_available(),
            CommandRequirement::InteractivePreview => {
                self.document().kind().is_typst() && self.interactive_preview_active()
            }
        }
    }

    pub(crate) fn take_window_request(&mut self) -> Option<EditorWindowRequest> {
        self.pending_window_requests.pop_front()
    }

    pub(crate) fn can_reuse_for_external_open(&self) -> bool {
        self.document().path().is_none() && !self.is_dirty() && !self.document_flow_busy()
    }

    pub(crate) fn open_external_path(&mut self, path: PathBuf) {
        self.queued_open_requests.push_back(path);
    }

    pub(crate) fn window_title(&self) -> String {
        self.title()
    }

    pub(crate) fn document_key(&self) -> DocumentKey {
        self.tabs
            .process_close_key
            .unwrap_or_else(|| self.document().key())
    }
    pub(crate) fn begin_process_close(&mut self) -> bool {
        if self.document_flow_busy() {
            return false;
        }
        self.process_close_pending = true;
        self.tabs.process_close_key = Some(self.document().key());
        self.document_workflow.revoke_close();
        self.request_document_replacement(
            DeferredDocumentAction::CloseWindow,
            "closing all document windows",
        );
        true
    }
    pub(crate) fn process_close_answer(&self) -> Option<bool> {
        if !self.process_close_pending {
            return None;
        }
        if self.close_accepted() {
            Some(true)
        } else if !self.document_flow_busy() {
            Some(false)
        } else {
            None
        }
    }
    pub(crate) fn finish_process_close(&mut self, accepted: bool) {
        self.process_close_pending = false;
        self.tabs.process_close_key = None;
        if !accepted {
            self.tabs.approved.clear();
            self.document_workflow.revoke_close();
        }
    }
    pub(crate) fn process_close_pending(&self) -> bool {
        self.process_close_pending
    }

    pub(crate) fn is_dirty_for_close(&self) -> bool {
        self.is_dirty()
            || self.tabs.ids().any(|id| {
                self.tabs.active_id() != Some(id)
                    && self
                        .document_for_tab(id)
                        .is_some_and(DocumentSession::is_dirty)
            })
    }

    pub(crate) fn close_accepted(&self) -> bool {
        self.document_workflow.may_close(self.document().key()) && self.tabs_close_approved()
    }

    pub(crate) fn finish_window_close(&mut self) {
        if !self.lifecycle.suspend() {
            return;
        }
        self.tabs = tabs::Tabs::default();
        let _ = self.compiler.pause(self.document().revision());
        self.stop_tinymist_session();
        self.preview.recovery.reset();
        self.preview.connection.suspend(false);
        self.preview.tinymist_state = ServiceState::Disabled("No document window is open".into());
        self.preview.webview_state = ServiceState::Disabled("No document window is open".into());
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.webview = None;
            self.webview_applied = None;
            self.webview_url = None;
            self.webview_navigation = None;
            self.webview_reload_pending = false;
        }
        self.compile_deadline = None;
        self.project_index_deadline.clear();
        self.project_index_job.supersede();
        self.workspace_service.unsubscribe();
        self.workspace = None;
        self.workspace_error = None;
        self.project_index = ProjectIndex::default();
        // Read results are discarded. Exclusive mutations detach to the
        // process completion mailbox instead of being canceled on close.
        self.git = Default::default();
        self.git_editor = Default::default();
        self.save_job = Default::default();
        self.pending_save = None;
        self.document_workflow.save_in_flight = false;
        self.asset_thumbnail_token.advance();
        self.asset_thumbnail_loader
            .cancel_before(self.asset_thumbnail_token);
        self.close_app_popup();
        self.diagnostic_tooltip = None;
        self.tooltip_request = None;
        // macOS retains the root native window as a process host. Its closed
        // document must not retain discarded edits or an autosave destination.
        self.reset_untitled_document();
        self.document_workflow.revoke_close();
    }

    pub(crate) fn request_window_resume(&mut self) {
        self.lifecycle.request_resume();
    }

    pub(crate) fn reuse_dormant_window(&mut self, request: EditorWindowRequest) {
        self.lifecycle.request_resume();
        match request {
            EditorWindowRequest::New { workspace_root } => {
                self.workspace_root = canonical_or_absolute(&workspace_root);
                self.remember_workspace(&self.workspace_root.clone());
                self.reset_untitled_document();
            }
            EditorWindowRequest::Open(path) if path.is_dir() => {
                self.workspace_root = canonical_or_absolute(&path);
                self.remember_workspace(&self.workspace_root.clone());
                self.tabs = self.tabs.empty_after();
            }
            EditorWindowRequest::Open(path) => self.open_external_path(path),
        }
    }

    pub(crate) fn needs_visible_window(&self) -> bool {
        self.lifecycle.needs_visible_window()
    }

    /// No painting or stale root input in eframe's hidden-window logic hook.
    pub(crate) fn hidden_host_logic(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        let _span = crate::performance::span("host.dormant.logic");
        ChildViewHost::dormant_owner(context);
        self.process_open_requests(context);
        self.execute_pending_document_action(context, frame);
        self.poll_export_dialog(context);
        self.poll_tool_picker(context);
        self.poll_document_dialog(context);
        self.poll_file_import();
        self.poll_package_catalog(context);
        self.poll_font_catalog(context);
        self.consume_settings_actions(context, frame);
        let was_visible = self.settings_visible;
        self.process_native_menu_commands(context, frame);
        if self.window_host.is_root() && self.take_settings_open_request() {
            self.open_global_settings(context);
        }
        if was_visible != self.settings_visible {
            let child = scoped_child_viewport_id(context, "tiptoptyp-settings");
            context
                .send_viewport_cmd_to(child, egui::ViewportCommand::Visible(self.settings_visible));
            if self.settings_visible {
                context.send_viewport_cmd_to(child, egui::ViewportCommand::Minimized(false));
                context.send_viewport_cmd_to(child, egui::ViewportCommand::Focus);
            }
        }
        // Drain canceled service results without accepting them or retrying.
        while self.compiler.try_recv().is_some() {}
        while self.asset_loader.try_recv().is_some() {}
        self.preview_page_loader.cancel();
        self.asset_page_loader.cancel();
        while self.preview_page_loader.try_recv().is_some() {}
        while self.asset_page_loader.try_recv().is_some() {}
        while self.asset_thumbnail_loader.try_recv().is_some() {}
        while self.tinymist.try_recv().is_some() {}
        self.record_notice_transition();
    }

    pub(crate) fn hidden_host_ui(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        let _span = crate::performance::span("host.dormant.ui");
        self.sync_runtime_settings(context);
        if owner_has_focused_viewport(context, context.viewport_id(), false) {
            self.handle_shortcuts(context, frame);
        }
        self.show_settings_window(context, frame);
        self.show_shortcut_editor_window(context);
        self.show_typst_overrides_window(context);
        self.show_workspace_chooser(context);
        self.show_package_manager_window(context);
        self.show_app_modal_window(context);
    }

    pub(crate) fn register_dormant_settings(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        // Once allocated as a process host, keep this native surface alive
        // across document resumes. Dropping it can detach the current CGL
        // view before eframe switches back to the root surface.
        self.retain_settings_viewport = true;
        self.show_settings_window(context, frame);
    }

    pub(crate) fn show_window_notice(&mut self, message: String) {
        self.notice = Some(Notice {
            message,
            kind: NoticeKind::Info,
        });
    }

    fn is_dirty(&self) -> bool {
        self.document().is_dirty()
    }

    fn document_name(&self) -> String {
        self.document().name()
    }

    fn title(&self) -> String {
        if self.tabs.is_empty() {
            return format!(
                "{} — tiptoptyp",
                self.workspace_root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            );
        }
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
        self.document()
            .path()
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
            self.font_catalog_scan.supersede();
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

    fn open_package_manager(&mut self, context: &egui::Context) {
        self.packages_visible = true;
        self.close_app_popup();
        if self.package_catalog.is_none() && !self.package_catalog_job.is_running() {
            self.request_package_catalog(context);
        }
    }

    fn request_package_catalog(&mut self, context: &egui::Context) {
        let roots = PackageRoots::standard();
        if self.package_catalog.is_none() {
            self.package_catalog = Some(PackageCatalogLoad::load_installed(&roots));
        }
        if let Err(error) = self.package_catalog_job.start_and_repaint(
            "tiptoptyp-package-catalog",
            context,
            move || Ok(PackageCatalogLoad::load(&roots)),
        ) {
            self.notice = Some(Notice {
                message: format!("Could not start package discovery: {error}"),
                kind: NoticeKind::Error,
            });
        }
    }

    fn poll_package_catalog(&mut self, context: &egui::Context) {
        match self.package_uninstall.poll() {
            LatestJobPoll::Ready(message) => {
                self.notice = Some(Notice {
                    message,
                    kind: NoticeKind::Success,
                });
                self.package_catalog = None;
                self.request_package_catalog(context);
            }
            LatestJobPoll::Failed(error) => {
                self.show_file_error(error);
                self.package_catalog = None;
                self.request_package_catalog(context);
            }
            LatestJobPoll::Pending | LatestJobPoll::Idle => {}
        }
        match self.package_catalog_job.poll() {
            LatestJobPoll::Ready(catalog) => self.package_catalog = Some(catalog),
            LatestJobPoll::Failed(error) => {
                self.notice = Some(Notice {
                    message: format!("Package discovery stopped: {error}"),
                    kind: NoticeKind::Error,
                });
            }
            LatestJobPoll::Pending => {
                context.request_repaint_after(Duration::from_millis(100));
            }
            LatestJobPoll::Idle => {}
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
        if self.snapshot_scene.is_some_and(|scene| {
            !matches!(
                scene,
                UiSnapshotScene::Tabs | UiSnapshotScene::TabsPdf | UiSnapshotScene::TabsImage
            )
        }) {
            return None;
        }
        if self.tabs.uses_designated_preview() {
            return self
                .tab_preview_document()
                .filter(|document| document.kind().is_typst())
                .map(|document| {
                    document.path().clone().unwrap_or_else(|| {
                        self.untitled_tab_path(
                            self.tabs.preview_id().expect("preview tab must exist"),
                        )
                    })
                });
        }
        None
    }

    fn preview_document_path(&self) -> PathBuf {
        self.designated_preview_path()
            .or_else(|| {
                (self.document().kind().is_typst())
                    .then(|| self.document().path().clone())
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
        !self.tabs.is_empty()
            && typst_preview_available_for(
                self.document().kind(),
                self.designated_preview_path().is_some(),
            )
    }

    fn current_is_preview_document(&self) -> bool {
        if self.tabs.uses_designated_preview() {
            return self.tabs.active_id() == self.tabs.preview_id()
                && self.document().kind().is_typst();
        }
        match &self.document().path() {
            Some(path) => same_path(path, &self.preview_document_path()),
            None => self.designated_preview_path().is_none() && self.document().kind().is_typst(),
        }
    }

    fn preview_document_source(&self) -> Result<String, String> {
        if self.tabs.uses_designated_preview()
            && let Some(document) = self
                .tab_preview_document()
                .filter(|document| document.kind().is_typst())
        {
            if document.config().is_none() {
                return Ok(document.source().clone());
            }
            return document
                .canonical_snapshot()
                .map(|snapshot| snapshot.source().to_owned())
                .map_err(|e| e.to_string());
        }
        let path = self.preview_document_path();
        if self.current_is_preview_document() {
            return self.canonical_document_source();
        }
        fs::read_to_string(&path)
            .map_err(|error| format!("Could not read preview entry {}: {error}", path.display()))
    }

    fn canonical_document_source(&self) -> Result<String, String> {
        if self.document().config().is_none() {
            return Ok(self.document().source().clone());
        }
        self.document()
            .canonical_snapshot()
            .map(|snapshot| snapshot.source().to_owned())
            .map_err(|error| error.to_string())
    }

    fn schedule_project_index(&mut self) {
        if !self.lifecycle.allows_document_work() || self.tabs.is_empty() {
            return;
        }
        self.project_index_deadline
            .schedule(Instant::now() + PROJECT_INDEX_DEBOUNCE);
    }

    fn rebuild_project_index(&mut self, context: &egui::Context) {
        self.project_index_deadline.clear();
        let main = self.preview_document_path();
        let root = self.tab_preview_root().to_owned();
        let mut overrides = BTreeMap::new();
        if let Err(()) = self.collect_tab_sources(&mut overrides) {
            self.project_index_job.supersede();
            return;
        }
        if let Err(error) = self.project_index_job.request(
            ProjectIndexInput {
                root,
                main,
                overrides,
            },
            crate::worker::RepaintTarget::current(context),
        ) {
            self.notice = Some(Notice {
                message: format!("Could not start project index: {error}"),
                kind: NoticeKind::Error,
            });
        }
    }

    fn tick_project_index(&mut self, context: &egui::Context) {
        match self.project_index_job.poll() {
            ProjectIndexPoll::Ready(index) if !self.project_index_deadline.is_pending() => {
                self.project_index = index;
            }
            ProjectIndexPoll::Failed(error) => {
                self.notice = Some(Notice {
                    message: format!("Project index stopped: {error}"),
                    kind: NoticeKind::Error,
                });
            }
            ProjectIndexPoll::Idle | ProjectIndexPoll::Pending | ProjectIndexPoll::Ready(_) => {}
        }
        let Some(remaining) = self.project_index_deadline.remaining(Instant::now()) else {
            return;
        };
        if remaining.is_zero() {
            self.rebuild_project_index(context);
        } else {
            context.request_repaint_after(remaining);
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
        if self.document_flow_busy() {
            return;
        }
        let active = self.tabs.active_id().expect("non-empty tab set");
        if self.open_tab_path(path, context) {
            let preview = self.tabs.active_id().expect("opened tab must be active");
            self.select_preview_tab(preview, context);
            self.activate_tab(active, context);
        }
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
        if self.document_mut().take_edit().is_none() {
            return;
        }
        self.manual_format_revision = None;
        self.format_request_key = None;
        self.format_when_tinymist_ready = None;
        self.tooltip_request = None;
        self.notice = None;
        self.editor_hover = None;
        self.editor_completion = None;
        if self.document().config().is_some() {
            // Mapped positions belong to the previous view revision.
            self.preview.diagnostics.clear();
            self.preview.tinymist_diagnostics.clear();
            self.mark_diagnostics_changed();
        }
        if self.document().kind().is_typst() {
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
            // Pausing freezes preview output, not the language server. Keep the
            // exact document version synchronized so completions, hover,
            // formatting, and diagnostics continue to describe this buffer.
            if let Err(error) = self.sync_tinymist_change() {
                self.preview.tinymist_state = ServiceState::Degraded(error);
            }
            self.schedule_project_index();
        } else {
            self.compile_deadline = None;
        }
        self.schedule_autosave_if_needed();
    }

    fn schedule_autosave_if_needed(&mut self) {
        let deadline = (self.snapshot_scene.is_none()
            && self.settings.auto_save
            && self.document().path().is_some()
            && self.document().is_dirty())
        .then(|| Instant::now() + Duration::from_millis(self.settings.auto_save_delay_ms.max(100)));
        self.set_active_autosave_deadline(deadline);
    }

    fn tick_autosave(&mut self, context: &egui::Context) {
        if self.save_job.is_running() {
            return;
        }
        if self.snapshot_scene.is_some() {
            self.set_active_autosave_deadline(None);
            return;
        }
        if self.document_workflow.has_dialog() || self.document_workflow.modal().is_some() {
            return;
        }
        let Some(deadline) = self.active_autosave_deadline() else {
            return;
        };
        if !self.settings.auto_save || !self.is_dirty() {
            self.set_active_autosave_deadline(None);
            return;
        }
        let now = Instant::now();
        if deadline > now {
            context.request_repaint_after(deadline - now);
            return;
        }
        self.set_active_autosave_deadline(None);
        let Some(path) = self.document().path().clone() else {
            return;
        };

        self.save_to_with_intent(path, SaveIntent::Auto, context);
    }

    fn editor_revision(&self) -> DocumentKey {
        self.document().key()
    }

    fn prepare_editor_source_data(&mut self) {
        let projected = self.document().config().is_some();
        self.editor_data.set_mitex_dollars(projected);
        self.highlighter.set_mitex_dollars(projected);
        self.editor_data.prepare_source(&self.document().snapshot());
    }

    fn prepare_editor_data(&mut self) {
        self.prepare_editor_source_data();
        let document = self.editor_revision();
        let current_path = self.document().path().clone();
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
        let source_dir = self.preview_source_directory();
        let display_name = preview_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.typ".to_owned());
        let request = CompileRequest {
            revision: self.document().revision(),
            rasterize: self.raster_preview_required(),
            source,
            project_root: self.tab_preview_root().to_owned(),
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

    fn preview_source_directory(&self) -> PathBuf {
        if self
            .tab_preview_document()
            .is_some_and(|document| document.kind().is_typst() && document.path().is_none())
        {
            return self.tab_preview_root().into();
        }
        if self.current_is_preview_document() && self.document().path().is_none() {
            // The preview path is only a virtual identity for an untitled
            // document. Relative imports and private mirrors need a real root.
            self.current_directory()
                .unwrap_or_else(|| self.project_root())
        } else {
            self.preview_document_path()
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.project_root())
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

    fn receive_compile_results(&mut self) {
        let _span = crate::performance::span("compile.receive");
        while let Some(result) = self.compiler.try_recv() {
            if !self.preview_processing_enabled()
                || !self.may_run_compilation()
                || result.revision != self.document().revision()
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
                CompileEvent::Catalog { key, catalog }
                    if self.preview.accepts_raster(key, self.document().revision()) =>
                {
                    self.preview.accept_catalog(key, catalog);
                }
                CompileEvent::RasterFailed { key, error }
                    if self.preview.accepts_raster(key, self.document().revision()) =>
                {
                    self.preview.content.fail_raster(key, error.clone());
                    self.notice = Some(Notice {
                        message: format!(
                            "Preview is ready, but its raster preview is unavailable: {error}"
                        ),
                        kind: NoticeKind::Error,
                    });
                }
                CompileEvent::Catalog { .. } | CompileEvent::RasterFailed { .. } => {}
            }
        }
    }

    fn receive_asset_results(&mut self, context: &egui::Context) {
        while let Some(result) = self.asset_loader.try_recv() {
            self.accept_asset_result(context, result);
        }
    }

    fn receive_pdf_page_results(&mut self, context: &egui::Context) {
        let mut results = Vec::new();
        while let Some(result) = self.preview_page_loader.try_recv() {
            results.push(result);
        }
        while let Some(result) = self.asset_page_loader.try_recv() {
            results.push(result);
        }
        let owner = self.document().key().owner;
        for result in results {
            let preview = match result.surface {
                PdfSurface::Document => &mut self.preview,
                PdfSurface::Asset => &mut self.asset_preview,
            };
            if !preview.accepts_page_key(result.key) {
                continue;
            }
            match result.output {
                Ok(mut pages) => {
                    let dark = preview.dark;
                    // Admit speculative neighbours first and visible pages
                    // last. If this batch crosses the process-wide budget,
                    // eviction keeps the pixels the user can actually see.
                    pages.sort_by_key(|(index, _)| preview.page_is_demanded(*index));
                    let mut residents = Vec::with_capacity(pages.len());
                    for (index, page) in pages {
                        let size = page.size;
                        let visible = preview.page_is_demanded(index);
                        let resident = make_preview_resident(
                            context,
                            owner,
                            PreviewResidentInput {
                                request: result.key,
                                index,
                                size,
                                rgba: page.rgba,
                                dark,
                                visible,
                            },
                        );
                        // Admission can evict an earlier page in this same
                        // result. Drop its decoded bytes and texture now rather
                        // than retaining the whole over-budget batch until the
                        // next frame.
                        residents.retain(|(_, resident): &(usize, ResidentPreviewTexture)| {
                            resident.lease.is_resident()
                        });
                        preview.prune_evicted_pages();
                        residents.push((index, resident));
                    }
                    preview.accept_page_residents(result.key, residents);
                }
                Err(error) => {
                    preview
                        .content
                        .fail_raster(result.key.artifact, error.clone());
                    self.notice = Some(Notice {
                        message: format!("PDF page preview is unavailable: {error}"),
                        kind: NoticeKind::Error,
                    });
                }
            }
        }
    }

    fn tick_pdf_page_requests(&mut self) {
        self.preview.prune_evicted_pages();
        self.asset_preview.prune_evicted_pages();

        let raster_document_visible = self.typst_preview_available()
            && self.view_mode.shows_preview()
            && (!self.interactive_preview_active() || self.captures.has_pending_for("main"));
        let asset_visible = self.document().kind().preview_only();
        let owner = self.document().key().owner;
        let visible = raster_document_visible
            .then(|| self.preview.visible_residency_ids())
            .into_iter()
            .flatten()
            .chain(
                asset_visible
                    .then(|| self.asset_preview.visible_residency_ids())
                    .into_iter()
                    .flatten(),
            )
            .collect::<Vec<_>>();
        crate::pdf_residency::set_owner_visible(owner, visible);

        if raster_document_visible {
            let root = self.tab_preview_root().to_path_buf();
            if let Err(error) = request_pdf_pages(
                &mut self.preview,
                &self.preview_page_loader,
                PdfSurface::Document,
                root,
            ) {
                self.notice = Some(Notice {
                    message: error,
                    kind: NoticeKind::Error,
                });
            }
        }
        if asset_visible {
            let root = self.project_root();
            if let Err(error) = request_pdf_pages(
                &mut self.asset_preview,
                &self.asset_page_loader,
                PdfSurface::Asset,
                root,
            ) {
                self.notice = Some(Notice {
                    message: error,
                    kind: NoticeKind::Error,
                });
            }
        }
    }

    fn accept_asset_result(&mut self, context: &egui::Context, result: crate::asset::AssetResult) {
        if result.token != self.asset_token || !self.document().kind().preview_only() {
            return;
        }
        match result.output {
            Ok(LoadedAsset::Image(page)) => {
                let key = ArtifactKey::unversioned(self.document().revision());
                let image_size = page.size;
                let mut texture =
                    make_preview_texture(context, self.document().key().owner, key, 0, page, false);
                // Native PDF pages are 144-DPI rasters interpreted in
                // 72-point coordinates. Doubling the image's logical
                // raster size cancels that conversion and makes 100% mean
                // one source image pixel per UI point.
                texture.size = [
                    image_size[0].saturating_mul(2),
                    image_size[1].saturating_mul(2),
                ];
                self.asset_preview.replace_asset(key, None, vec![texture]);
            }
            Ok(LoadedAsset::Pdf { bytes, catalog }) => {
                let key = ArtifactKey::unversioned(self.document().revision());
                self.asset_preview
                    .replace_pdf_asset(key, bytes.into(), catalog);
                if let Some(page) = self.pending_asset_page.take() {
                    self.asset_preview.requested_page =
                        Some(page.min(self.asset_preview.content.pages().len().saturating_sub(1)));
                }
                self.complete_pending_export();
            }
            Err(error) => {
                if !self.typst_preview_available() {
                    self.document_workflow.pending_export = None;
                }
                self.asset_preview.status = PreviewStatus::Error;
                self.notice = Some(Notice {
                    message: error,
                    kind: NoticeKind::Error,
                });
            }
        }
    }

    fn receive_asset_thumbnail_results(&mut self, context: &egui::Context) {
        while let Some(outcome) = self.asset_thumbnail_loader.try_recv() {
            let result = match outcome {
                Ok(result) => result,
                Err(error) => {
                    if let Some(hover) = &mut self.asset_hover {
                        hover.content = AssetHoverContent::Error(error);
                    }
                    continue;
                }
            };
            if !asset_thumbnail_result_matches(self.asset_hover.as_ref(), &result) {
                continue;
            }
            let content = match result.output {
                Ok(thumbnail) => {
                    let expected_bytes = thumbnail.size[0]
                        .checked_mul(thumbnail.size[1])
                        .and_then(|pixels| pixels.checked_mul(4));
                    if expected_bytes != Some(thumbnail.rgba.len()) {
                        AssetHoverContent::Error(
                            "The decoded asset thumbnail had invalid pixel data".to_owned(),
                        )
                    } else {
                        let image = ColorImage::from_rgba_unmultiplied(
                            thumbnail.size,
                            thumbnail.rgba.as_ref(),
                        );
                        let texture = context.load_texture(
                            format!("asset-hover-{}", result.token),
                            image,
                            TextureOptions::LINEAR,
                        );
                        AssetHoverContent::Ready {
                            texture,
                            source_size: thumbnail.size,
                        }
                    }
                }
                Err(error) => AssetHoverContent::Error(error),
            };
            if let Some(hover) = &mut self.asset_hover {
                hover.content = content;
            }
        }
    }

    fn update_asset_hover(&mut self, context: &egui::Context) {
        if self.snapshot_scene == Some(UiSnapshotScene::AssetPreview) {
            return;
        }
        let candidate = current_asset_hover_candidate(context);
        let Some(candidate) = candidate else {
            if !native_tooltip_handoff_active(context, false) {
                self.clear_asset_hover();
            }
            return;
        };
        if native_tooltip_handoff_blocks(context, candidate.origin) {
            return;
        }

        self.diagnostic_tooltip = None;
        self.editor_hover = None;
        clear_native_hover_overlay(context);

        let target_changed = self
            .asset_hover
            .as_ref()
            .is_none_or(|hover| hover.path != candidate.path || hover.kind != candidate.kind);
        if target_changed {
            self.asset_thumbnail_token.advance();
            let token = self.asset_thumbnail_token;
            let request =
                self.asset_thumbnail_loader
                    .request(token, candidate.path.clone(), candidate.kind);
            self.asset_hover = Some(AssetHoverState {
                origin: candidate.origin,
                anchor: candidate.anchor,
                placement: candidate.placement,
                path: candidate.path,
                kind: candidate.kind,
                opacity: candidate.opacity,
                token,
                content: request
                    .map_or_else(AssetHoverContent::Error, |_| AssetHoverContent::Loading),
            });
        } else if let Some(hover) = &mut self.asset_hover {
            hover.origin = candidate.origin;
            hover.anchor = candidate.anchor;
            hover.placement = candidate.placement;
            hover.opacity = candidate.opacity;
        }
    }

    fn clear_asset_hover(&mut self) {
        if self.asset_hover.take().is_some() {
            self.asset_thumbnail_token.advance();
            self.asset_thumbnail_loader
                .cancel_before(self.asset_thumbnail_token);
        }
    }

    fn set_compile_error(&mut self, error: String) {
        self.preview.content.invalidate();
        self.preview.status = PreviewStatus::Error;
        self.set_diagnostics(error);
    }

    fn set_diagnostics(&mut self, raw: String) {
        let display_name = self
            .preview_document_path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.document_name());
        let mut diagnostics = parse_typst_short_output(&raw, Some(Path::new(&display_name)));
        if self.document().config().is_some()
            && let Ok(snapshot) = self.document().canonical_snapshot()
        {
            for diagnostic in &mut diagnostics {
                if self.diagnostic_targets_current_document(diagnostic) {
                    diagnostic.location = diagnostic.location.and_then(|location| {
                        let cursor = char_index_at_line_column(
                            snapshot.source(),
                            location.line,
                            location.column,
                        );
                        let cursor = snapshot.editor_scalar_cursor(ScalarOffset::new(cursor))?;
                        let (line, column) =
                            line_column_at_char(snapshot.editor_source(), cursor.get());
                        Some(crate::diagnostics::DiagnosticLocation { line, column })
                    });
                }
            }
        }
        self.preview.set_diagnostics(raw, diagnostics);
    }

    fn schedule_compile_now(&mut self) {
        let transition = self
            .preview
            .transition(PreviewTransitionEvent::RenderRequested);
        self.apply_preview_transition(transition, None);
    }

    fn schedule_compile_now_io(&mut self) {
        if !self.lifecycle.allows_document_work()
            || !self.preview_processing_enabled()
            || !self.may_run_compilation()
        {
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
                if let Err(error) = self.compiler.pause(self.document().revision()) {
                    self.compilation_paused = false;
                    self.notice = Some(Notice {
                        message: format!("Could not pause the Typst watcher: {error}"),
                        kind: NoticeKind::Error,
                    });
                    return;
                }
            }
            let transition = self
                .preview
                .transition(PreviewTransitionEvent::PauseChanged {
                    generation: self.tinymist_sync.generation,
                    paused: true,
                });
            self.apply_preview_transition(transition, None);
            let (message, kind) = compilation_notice(true);
            self.notice = Some(Notice {
                message: message.to_owned(),
                kind,
            });
        } else {
            let transition = self
                .preview
                .transition(PreviewTransitionEvent::PauseChanged {
                    generation: self.tinymist_sync.generation,
                    paused: false,
                });
            self.apply_preview_transition(transition, None);
            let transition = self
                .preview
                .transition(PreviewTransitionEvent::RenderRequested);
            self.apply_preview_transition(transition, None);
            let (message, kind) = compilation_notice(false);
            self.notice = Some(Notice {
                message: message.to_owned(),
                kind,
            });
        }
    }

    fn compile_pdf(&mut self, frame: Option<&eframe::Frame>) {
        if !self.typst_preview_available() {
            self.notice = Some(Notice {
                message: "Compile needs a Typst preview entry".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        let designated = self.designated_preview_path();
        let destination = if self.tabs.uses_designated_preview() {
            self.tab_preview_document().and_then(|document| {
                default_compile_pdf_path(None, document.kind(), document.path().as_deref())
            })
        } else {
            default_compile_pdf_path(
                designated.as_deref(),
                self.document().kind(),
                self.document().path().as_deref(),
            )
        };
        let Some(path) = destination else {
            // An unsaved Typst document has no meaningful adjacent output
            // path, so ask once where its first PDF should be written.
            self.choose_pdf_output(frame, PdfWriteIntent::Compile);
            return;
        };
        self.finish_pdf_output(path, PdfWriteIntent::Compile);
    }

    fn clear_preview_for_document(&mut self, preserve_designated_preview: bool) {
        self.manual_format_revision = None;
        self.format_request_key = None;
        self.format_when_tinymist_ready = None;
        self.external_file_change_notice = None;
        self.external_file_stamp = self
            .document()
            .path()
            .as_deref()
            .and_then(|path| external_file_stamp(path).ok());
        self.asset_token.advance();
        self.asset_loader.cancel_before(self.asset_token);
        self.preview_page_loader.cancel();
        self.asset_page_loader.cancel();
        self.asset_preview
            .clear_for_document(self.document().revision(), false);
        self.preview
            .clear_for_document(self.document().revision(), preserve_designated_preview);
        self.pending_asset_page = None;
        self.editor_hover = None;
        self.clear_asset_hover();
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

    fn handle_shortcuts(&mut self, context: &egui::Context, frame: Option<&eframe::Frame>) {
        let settings_keys = self.settings_window.lock().unwrap().take_owner_keys();
        if !settings_keys.is_empty() {
            let viewport = scoped_child_viewport_id(context, "tiptoptyp-settings");
            context.input_mut_for(viewport, |input| input.events.extend(settings_keys));
        }
        let shortcut_viewport = focused_input_viewport(context);
        if self.handle_shortcut_capture(context, shortcut_viewport) {
            return;
        }
        let shortcuts = self.settings.effective_shortcuts();
        self.handle_extra_shortcuts(context, shortcut_viewport, &shortcuts);
        // egui allows extra Shift/Alt modifiers on a simpler shortcut. Consume
        // Cmd+Shift+W before the File menu's Cmd+W tab action.
        let child_focused = shortcut_viewport != context.viewport_id();
        if context.input_mut_for(shortcut_viewport, |input| {
            tabs::consume_window_close(input, &shortcuts, child_focused)
        }) {
            context.send_viewport_cmd_to(shortcut_viewport, egui::ViewportCommand::Close);
        }
        if shortcut_viewport == context.viewport_id()
            && !self.tabs.is_empty()
            && !self.document_flow_busy()
            && !self.process_close_pending
            && let Some(action) = context.input_mut(|input| {
                consume_shortcut_action(input, &shortcuts, |action| {
                    matches!(
                        action,
                        ShortcutAction::NextTab | ShortcutAction::PreviousTab
                    )
                })
            })
        {
            let count = self.tabs.len();
            let active = self.tabs.active_index().expect("non-empty tab set");
            let next_index = if action == ShortcutAction::PreviousTab {
                (active + count - 1) % count
            } else {
                (active + 1) % count
            };
            let next = self.tabs.id_at(next_index).unwrap();
            self.activate_tab(next, context);
        }
        if shortcut_viewport == context.viewport_id()
            || (shortcut_viewport == scoped_child_viewport_id(context, "tiptoptyp-popup-overlay")
                && matches!(self.app_popup, Some(AppPopup::GitChunk { .. })))
        {
            self.handle_hunk_shortcuts(context, shortcut_viewport, &shortcuts);
        }
        // Age the request on the owning document viewport's frame clock. The
        // target may be a child viewport that closes before clipboard delivery;
        // consulting a removed child's clock would panic in debug builds and
        // would not provide a bounded lifetime if that child stops rendering.
        let owner_frame = context.cumulative_frame_nr();
        let allow_requested_paste = self
            .pending_widget_paste
            .is_some_and(|pending| pending.admits(shortcut_viewport, owner_frame));
        let requested_paste_admitted = context.input_mut_for(shortcut_viewport, |input| {
            normalize_text_edit_shortcut_events(input, &shortcuts, allow_requested_paste)
        });
        let pending_paste_expired = self
            .pending_widget_paste
            .is_some_and(|pending| pending.expired(owner_frame));
        if requested_paste_admitted || pending_paste_expired {
            self.pending_widget_paste = None;
        }
        let editor_id = source_editor_id(context);
        let source_focused = context.memory(|memory| memory.focused()) == Some(editor_id);
        self.handle_editor_completion_keys(context);
        let completion_requested = source_focused
            && shortcuts
                .egui(ShortcutAction::Complete)
                .is_some_and(|shortcut| {
                    context
                        .input_mut_for(shortcut_viewport, |input| input.consume_shortcut(&shortcut))
                });
        if completion_requested {
            let snapshot = self.editor_snapshot(context);
            if snapshot.cursor.primary.index == snapshot.cursor.secondary.index {
                let cursor = snapshot.cursor.primary.index.0;
                let key = self.document().key();
                let anchor = self
                    .last_editor_caret
                    .filter(|caret| caret.key == key && caret.char_index == cursor)
                    .map(|caret| caret.rect)
                    .unwrap_or_else(|| {
                        context
                            .input(|input| input.viewport().inner_rect)
                            .map(|rect| Rect::from_center_size(rect.center(), Vec2::splat(1.0)))
                            .unwrap_or_else(|| Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.0)))
                    });
                self.request_editor_completion(cursor, anchor, true);
            } else {
                self.editor_completion = None;
                self.notice = Some(Notice {
                    message: "Place the caret at one position to request completions".to_owned(),
                    kind: NoticeKind::Info,
                });
            }
        }
        let other_text_input_focused = !source_focused && context.egui_wants_keyboard_input();
        let can_sync_preview =
            self.document().kind().is_typst() && self.interactive_preview_active();
        let global_command = context.input_mut_for(shortcut_viewport, |input| {
            consume_shortcut(input, &shortcuts, |command| {
                matches!(
                    command_spec(command).menu,
                    CommandMenu::Application | CommandMenu::File | CommandMenu::View
                ) || matches!(command, AppCommand::Find | AppCommand::FindReplace)
                    || (can_sync_preview && command == AppCommand::SyncPreview)
                    || (!other_text_input_focused
                        && matches!(command, AppCommand::Format | AppCommand::ToggleComment))
            })
        });
        if let Some(command) = global_command {
            self.execute_app_command(command, context, frame);
        }
        if shortcuts
            .egui(ShortcutAction::Compile)
            .is_some_and(|shortcut| {
                context.input_mut_for(shortcut_viewport, |input| input.consume_shortcut(&shortcut))
            })
        {
            self.compile_pdf(frame);
        }
        if shortcuts
            .egui(ShortcutAction::ToggleCompilation)
            .is_some_and(|shortcut| {
                context.input_mut_for(shortcut_viewport, |input| input.consume_shortcut(&shortcut))
            })
        {
            self.toggle_compilation_paused();
        }
        let edit_command = context.input_mut_for(shortcut_viewport, |input| {
            consume_shortcut(input, &shortcuts, |command| {
                matches!(
                    command,
                    AppCommand::Undo
                        | AppCommand::Redo
                        | AppCommand::Cut
                        | AppCommand::Copy
                        | AppCommand::Paste
                        | AppCommand::SelectAll
                )
            })
        });
        if self.tooltip_request.is_some_and(|request| match request {
            TooltipRequest::Caret { key, cursor } => {
                key != self.document().key()
                    || self
                        .last_editor_caret
                        .is_some_and(|caret| caret.char_index != cursor)
            }
            TooltipRequest::Pointer(position) => context.pointer_latest_pos() != Some(position),
        }) {
            self.tooltip_request = None;
        }
        for action in [ShortcutAction::PointerTooltip, ShortcutAction::CaretTooltip] {
            if shortcuts.egui(action).is_some_and(|shortcut| {
                context.input_mut_for(shortcut_viewport, |input| input.consume_shortcut(&shortcut))
            }) {
                let interaction_id = tooltip_interaction_id(context);
                let focused = context
                    .data(|data| data.get_temp::<TooltipInteractionState>(interaction_id))
                    .is_some_and(|state| state.focused || state.focus_requested);
                if focused || self.tooltip_request.is_some() {
                    self.dismiss_keyboard_tooltip(context);
                } else {
                    self.clear_asset_hover();
                    self.diagnostic_tooltip = None;
                    clear_native_hover_overlay(context);
                    let dismissed_id = viewport_scoped_id(context, "dismissed-tooltip-origin");
                    let geometry_id = tooltip_geometry_id(context);
                    context.data_mut(|data| {
                        data.remove::<Rect>(dismissed_id);
                        data.remove::<TooltipGeometry>(geometry_id);
                        data.remove::<TooltipInteractionState>(interaction_id);
                    });
                    self.tooltip_request = match action {
                        ShortcutAction::CaretTooltip => {
                            let cursor = self.editor_snapshot(context).cursor.primary.index.0;
                            Some(TooltipRequest::Caret {
                                key: self.document().key(),
                                cursor,
                            })
                        }
                        _ => context.pointer_latest_pos().map(TooltipRequest::Pointer),
                    };
                    context.request_repaint();
                }
            }
        }
        if self.tooltip_request.is_some()
            && context.input_mut_for(shortcut_viewport, |input| {
                input.consume_key(Modifiers::NONE, egui::Key::Escape)
            })
        {
            self.dismiss_keyboard_tooltip(context);
        }
        if self.find_visible
            && shortcut_viewport == context.viewport_id()
            && context.input_mut_for(shortcut_viewport, |input| {
                input.consume_key(Modifiers::NONE, egui::Key::Escape)
            })
        {
            self.find_visible = false;
            self.replace_visible = false;
            self.search.clear();
        }

        if self.preview_visible()
            && (self.document().kind().preview_only() || !self.interactive_preview_active())
        {
            let preview = if self.document().kind().preview_only() {
                &mut self.asset_preview
            } else {
                &mut self.preview
            };
            match context.input_mut_for(shortcut_viewport, |input| {
                consume_preview_zoom_shortcut(input, &shortcuts)
            }) {
                Some(PreviewZoomAction::In) => {
                    preview.requested_zoom = Some(
                        (preview.zoom * METRICS.preview.zoom_step)
                            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                    );
                    preview.fit_width = false;
                }
                Some(PreviewZoomAction::Out) => {
                    preview.requested_zoom = Some(
                        (preview.zoom / METRICS.preview.zoom_step)
                            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                    );
                    preview.fit_width = false;
                }
                Some(PreviewZoomAction::Reset) => {
                    preview.requested_zoom = Some(1.0);
                    preview.fit_width = false;
                }
                None => {}
            }
        }

        if let Some(delta) = context.input_mut_for(shortcut_viewport, |input| {
            consume_ui_scale_shortcut(input, &shortcuts)
        }) {
            self.adjust_ui_scale(delta, context);
        }

        if shortcuts
            .egui(ShortcutAction::Minimize)
            .is_some_and(|shortcut| {
                context.input_mut_for(shortcut_viewport, |input| input.consume_shortcut(&shortcut))
            })
        {
            context.send_viewport_cmd_to(shortcut_viewport, egui::ViewportCommand::Minimized(true));
        }
        if shortcuts
            .egui(ShortcutAction::ToggleFullscreen)
            .is_some_and(|shortcut| {
                context.input_mut_for(shortcut_viewport, |input| input.consume_shortcut(&shortcut))
            })
        {
            let fullscreen = context
                .input_for(shortcut_viewport, |input| input.viewport().fullscreen)
                .unwrap_or(false);
            context.send_viewport_cmd_to(
                shortcut_viewport,
                egui::ViewportCommand::Fullscreen(!fullscreen),
            );
        }
        if shortcuts
            .egui(ShortcutAction::CaptureUi)
            .is_some_and(|shortcut| {
                context.input_mut_for(shortcut_viewport, |input| input.consume_shortcut(&shortcut))
            })
        {
            self.captures.queue_for_viewport(shortcut_viewport);
        }

        // Consume any effective binding which is intentionally unavailable in
        // the current focus/context so it cannot masquerade as a TextEdit
        // built-in. Then remove disabled platform editing chords as well.
        context.input_mut_for(shortcut_viewport, |input| {
            consume_shortcut_action(input, &shortcuts, |_| true);
            remove_unhandled_text_edit_builtin_events(input);
        });

        if let Some(command) = edit_command {
            if other_text_input_focused {
                self.route_edit_command_to_focused_widget(command, context, shortcut_viewport);
            } else if source_focused || !context.egui_wants_keyboard_input() {
                self.execute_app_command(command, context, frame);
            }
        }
    }

    /// Shortcut capture owns the next key event in whichever native viewport
    /// is active. Consuming it before normal routing prevents the old binding
    /// from firing and keeps the key out of an unrelated text field.
    fn handle_shortcut_capture(
        &mut self,
        context: &egui::Context,
        viewport: egui::ViewportId,
    ) -> bool {
        let Some(action) = self.shortcut_capture else {
            return false;
        };
        let event = context.input_mut_for(viewport, take_shortcut_capture_event);
        let Some((key, modifiers)) = event else {
            return true;
        };

        let mut edited = self
            .pending_settings
            .clone()
            .unwrap_or_else(|| self.settings.clone());
        if key == egui::Key::Escape {
            self.shortcut_capture = None;
            self.shortcut_notice = Some("Shortcut capture canceled".to_owned());
        } else if key == egui::Key::Backspace && !modifiers.any() {
            edited.shortcut_overrides.set(action, None);
            self.shortcut_capture = None;
            self.shortcut_notice = Some(format!("Disabled {}", action.label()));
        } else {
            match ShortcutChord::from_egui(
                KeyboardShortcut::new(modifiers, key),
                ShortcutPlatform::current(),
            ) {
                Ok(chord) => {
                    let displaced = edited.shortcut_overrides.assign(
                        action,
                        chord,
                        ShortcutPlatform::current(),
                    );
                    self.shortcut_capture = None;
                    self.shortcut_notice = Some(displaced.map_or_else(
                        || format!("Changed {}", action.label()),
                        |displaced| {
                            format!(
                                "Changed {}; {} was unassigned to avoid a conflict",
                                action.label(),
                                displaced.label()
                            )
                        },
                    ));
                }
                Err(error) => self.shortcut_notice = Some(error.to_string()),
            }
        }
        self.queue_settings(edited, context);
        true
    }

    fn execute_app_command(
        &mut self,
        command: AppCommand,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        if command_spec(command).menu == CommandMenu::File && self.document_flow_busy() {
            self.notice = Some(Notice {
                message: "Finish the current file operation before starting another".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        if !self.native_command_enabled(command) {
            return;
        }
        match command {
            AppCommand::Settings => self.toggle_settings(),
            AppCommand::CloseTab => {
                if let Some(id) = self.tabs.active_id() {
                    self.request_close_tab(id, context);
                }
            }
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
                self.save_document(frame, context);
            }
            AppCommand::SaveAs => {
                self.save_as(frame);
            }
            AppCommand::Rename => {
                if let Some(path) = self.document().path().clone() {
                    self.begin_rename(path);
                }
            }
            AppCommand::Packages => self.open_package_manager(context),
            AppCommand::Git => {
                let opens_explorer = git_command_opens_explorer(self.git.visible);
                if self.git.visible {
                    self.git.visible = false;
                    self.git_explorer_reveal = false;
                } else {
                    self.git.open(context, &self.workspace_root);
                    if opens_explorer {
                        // Git is an Explorer subpanel in normal windows. Make
                        // the owning panel visible as part of the command so
                        // invoking View > Git cannot produce an invisible
                        // state when Explorer was previously closed.
                        self.explorer.open();
                        self.git_explorer_reveal = true;
                    }
                }
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
            AppCommand::Explorer => {
                self.explorer.toggle();
                context.request_repaint();
            }
            AppCommand::Code => self.view_mode = ViewMode::Code,
            AppCommand::Split => self.view_mode = ViewMode::Split,
            AppCommand::Preview => self.view_mode = ViewMode::Preview,
        }
    }

    fn process_native_menu_commands(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        let focused_viewport = focused_input_viewport(context);
        while let Some(command) = self.queued_native_menu_commands.pop() {
            if self.route_edit_command_to_focused_widget(command, context, focused_viewport) {
                continue;
            }
            self.execute_app_command(command, context, frame);
        }
    }

    fn route_edit_command_to_focused_widget(
        &mut self,
        command: AppCommand,
        context: &egui::Context,
        viewport: egui::ViewportId,
    ) -> bool {
        if viewport == scoped_child_viewport_id(context, "tiptoptyp-settings")
            && self
                .settings_window
                .lock()
                .unwrap()
                .queue_edit_command(command)
        {
            // Input injected into the previous child pass would be discarded
            // before its next independent paint. Deliver inside that callback.
            context.request_repaint_of(viewport);
            return true;
        }
        let focused = context.memory(|memory| memory.focused());
        if focused == Some(source_editor_id(context)) || !context.egui_wants_keyboard_input() {
            return false;
        }
        match command {
            AppCommand::Cut => context.input_mut_for(viewport, |input| {
                input.events.push(egui::Event::Cut);
            }),
            AppCommand::Copy => context.input_mut_for(viewport, |input| {
                input.events.push(egui::Event::Copy);
            }),
            AppCommand::Paste => self.request_widget_paste(context, viewport),
            AppCommand::Undo | AppCommand::Redo | AppCommand::SelectAll => {
                if let Some(shortcut) = standard_text_edit_shortcut(command) {
                    context.input_mut_for(viewport, |input| {
                        input.events.push(egui::Event::Key {
                            key: shortcut.logical_key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: shortcut.modifiers,
                        });
                    });
                }
            }
            // Source-only editing commands must not mutate the document while
            // a find, settings, rename, package, or table field owns focus.
            AppCommand::ToggleComment | AppCommand::Format => {}
            _ => return false,
        }
        true
    }

    fn request_widget_paste(&mut self, context: &egui::Context, viewport: egui::ViewportId) {
        self.pending_widget_paste = Some(PendingWidgetPaste::new(
            viewport,
            context.cumulative_frame_nr(),
        ));
        context.send_viewport_cmd_to(viewport, egui::ViewportCommand::RequestPaste);
    }

    fn document_flow_busy(&self) -> bool {
        self.document_workflow.is_busy(
            self.rename_dialog.is_some() || self.table_editor.is_some(),
            self.pending_tool_picker.is_some(),
        )
    }

    fn set_settings_visible(&mut self, visible: bool) {
        self.settings_visible = visible;
        if visible {
            // A root popup and a child settings window must never compete for
            // native focus. Settings owns transient interaction until closed.
            self.close_app_popup();
        }
    }

    fn toggle_settings(&mut self) {
        self.settings_open_requested = true;
        self.close_app_popup();
    }

    pub(crate) fn take_settings_open_request(&mut self) -> bool {
        std::mem::take(&mut self.settings_open_requested)
    }

    pub(crate) fn open_global_settings(&mut self, context: &egui::Context) {
        self.retain_settings_viewport = true;
        self.set_settings_visible(true);
        let child =
            crate::child_view::child_viewport_id(egui::ViewportId::ROOT, "tiptoptyp-settings");
        context.send_viewport_cmd_to(child, egui::ViewportCommand::Visible(true));
        context.send_viewport_cmd_to(child, egui::ViewportCommand::Minimized(false));
        context.send_viewport_cmd_to(child, egui::ViewportCommand::Focus);
        context.request_repaint_of(egui::ViewportId::ROOT);
        context.request_repaint_of(child);
    }

    fn open_app_popup(&mut self, popup: AppPopup) {
        self.app_popup_generation = self.app_popup_generation.wrapping_add(1);
        self.app_popup = Some(popup);
        self.app_popup_had_focus = false;
        self.app_popup_blur_started = None;
    }

    fn close_app_popup(&mut self) {
        let closes_git_chunk = matches!(self.app_popup, Some(AppPopup::GitChunk { .. }));
        self.app_popup = None;
        self.app_popup_had_focus = false;
        self.app_popup_blur_started = None;
        if closes_git_chunk {
            self.git_editor.chunk = None;
        }
    }

    fn open_find(&mut self, replace: bool) {
        self.editor_completion = None;
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
        let ui_scale_percent = edited.ui_scale_percent;
        self.queue_settings(edited, context);
        apply_ui_scale(context, ui_scale_percent);
        self.presentation.record_ui_scale(ui_scale_percent);
    }

    fn queue_settings(&mut self, settings: AppSettings, context: &egui::Context) {
        if settings == self.settings {
            self.pending_settings = None;
        } else {
            self.pending_settings = Some(settings);
            context.request_repaint();
        }
    }

    fn prepare_qa_scene(&mut self, context: &egui::Context) {
        let mut qa = std::mem::take(&mut self.qa);
        qa.prepare(self, context);
        self.qa = qa;
    }

    pub(crate) fn set_capture_step(&mut self, step: &UiCaptureStep, context: &egui::Context) {
        let mut qa = std::mem::take(&mut self.qa);
        qa.set_step(self, step, context);
        self.qa = qa;
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
                theme::syntax_palette_from_semantic(active.palette),
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
            self.preview.bump_appearance(next_preview_dark);
            self.asset_preview.bump_appearance(
                next_preview_dark && self.document().kind() != DocumentKind::Image,
            );
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

        if refresh_tools {
            self.capabilities.refresh_tools();
        } else if changes.typst || changes.tinymist {
            self.capabilities.invalidate();
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
        let _span = crate::performance::span("theme.reload");
        let (active, error) = load_active_theme_or_fallback(request);
        if (context.theme() == egui::Theme::Dark) != active.dark_mode {
            crate::viewport_fonts::appearance_changed(context);
        }
        theme::set_imported_palette(context, Some((active.dark_mode, active.palette)));
        theme::configure_styles(context);
        self.generic_highlighter
            .set_custom_theme(Some(active.syntect_theme.clone()));
        self.highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::syntax_palette_from_semantic(active.palette),
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

    fn poll_file_import(&mut self) {
        match self.file_import.poll() {
            LatestJobPoll::Ready(message) => {
                self.notice = Some(Notice {
                    message,
                    kind: NoticeKind::Success,
                });
                self.refresh_workspace();
            }
            LatestJobPoll::Failed(message) => {
                self.show_file_error(message);
                self.refresh_workspace();
            }
            LatestJobPoll::Pending | LatestJobPoll::Idle => {}
        }
    }

    fn handle_dropped_file(&mut self, context: &egui::Context) {
        self.poll_file_import();
        if self.document_flow_busy() || self.file_import.is_running() {
            return;
        }
        let paths = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect::<Vec<_>>()
        });
        if paths.is_empty() {
            return;
        }
        let id = viewport_scoped_id(context, "file-drop-target");
        let target = context.data(|data| data.get_temp::<FileDropTarget>(id));
        match target {
            Some(FileDropTarget::Editor) => {
                self.queued_open_requests.extend(paths);
                context.request_repaint();
            }
            Some(FileDropTarget::Folder(directory)) => {
                let root = self.workspace_root.clone();
                if let Err(error) =
                    self.file_import
                        .start_and_repaint("file-import", context, move || {
                            let mut imported = 0;
                            let mut errors = Vec::new();
                            for path in paths {
                                match crate::workspace::WorkspaceRoot::open(&root)
                                    .and_then(|root| root.directory(&directory))
                                    .and_then(|directory| directory.import(&path))
                                {
                                    Ok(_) => imported += 1,
                                    Err(error) => {
                                        errors.push(format!("{}: {error}", path.display()))
                                    }
                                }
                            }
                            let message =
                                format!("Imported {imported} file(s) into {}", directory.display());
                            if errors.is_empty() {
                                Ok(message)
                            } else {
                                Err(format!("{message}\n{}", errors.join("\n")))
                            }
                        })
                {
                    self.show_file_error(error);
                }
            }
            None => {}
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
        if self.process_close_pending {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        let close_requested = context.input(|input| input.viewport().close_requested());
        if close_request_requires_confirmation(
            close_requested,
            self.is_dirty_for_close(),
            self.close_accepted(),
            self.snapshot_scene.is_some(),
        ) {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.document_workflow.modal().is_none() {
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
        let preserve_preview =
            self.tabs.len() > 1 && self.tabs.active_id() != self.tabs.preview_id();
        let record = self.tabs.current_record_mut();
        reset_untitled_buffer(&mut record.document, &mut record.autosave);
        if self.settings.mitex_auto_enable {
            self.document_mut().replace_unprojected_untitled("");
            self.activate_preferred_tex();
        }
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.clear_preview_for_document(preserve_preview);
        self.search.clear();
        if preserve_preview {
            // A new editor tab must not restart the pinned preview's services.
            if let Err(error) = self.prepare_tab_backings() {
                self.show_file_error(error);
                return;
            }
            let path = self.tinymist_document_path();
            self.reopen_tinymist_current_document(&path, DocumentKind::Typst);
            self.git_editor.clear_document();
            self.git_editor.request_refresh();
            return;
        }
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

    fn native_file_dialog(&mut self, frame: Option<&eframe::Frame>) -> Option<AsyncFileDialog> {
        let dialog = AsyncFileDialog::new();

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(parent) = self.native_window_parent.as_ref() {
            return Some(dialog.set_parent(parent));
        }

        if self.window_host.is_root()
            && let Some(frame) = frame
        {
            return Some(dialog.set_parent(frame));
        }

        self.show_file_error(
            "The active document window is not available yet; focus it and try again".to_owned(),
        );
        None
    }

    fn settings_file_dialog(&mut self, _frame: Option<&eframe::Frame>) -> Option<AsyncFileDialog> {
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
            Some(if let Some(frame) = _frame {
                dialog.set_parent(frame)
            } else {
                dialog
            })
        }
    }

    fn start_open_folder_dialog(&mut self, frame: Option<&eframe::Frame>) {
        if self.document_workflow.has_dialog() {
            return;
        }
        let Some(dialog) = self.native_file_dialog(frame) else {
            return;
        };
        let mut dialog = dialog.set_title("Open project folder");
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.document_workflow.start_dialog(PendingDialog::new(
            DocumentDialogRequest {
                target: DocumentDialogTarget::OpenFolder,
                key: self.document().key(),
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
            .document()
            .path()
            .as_ref()
            .is_some_and(|path| path.starts_with(&root))
        {
            if let Some(path) = self.document().path().clone() {
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

    fn forget_workspace(&mut self, root: &Path) {
        self.settings.forget_workspace(root);
        if let Some(settings) = &mut self.pending_settings {
            settings.forget_workspace(root);
        }
        self.workspace_history_removals
            .push(normalize_workspace_root(root));
    }

    fn open_dialog(&mut self) {
        self.request_document_replacement(
            DeferredDocumentAction::OpenFileDialog,
            "opening another document",
        );
    }

    fn open_in_new_window_dialog(&mut self, frame: Option<&eframe::Frame>) {
        if self.document_workflow.has_dialog() {
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
            .set_title("Open document or folder in new window");
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.document_workflow.start_dialog(PendingDialog::new(
            DocumentDialogRequest {
                target: DocumentDialogTarget::OpenFileInNewWindow,
                key: self.document().key(),
            },
            dialog.pick_file_or_folder(),
        ));
    }

    fn start_open_dialog(&mut self, frame: Option<&eframe::Frame>) {
        if self.document_workflow.has_dialog() {
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
        self.document_workflow.start_dialog(PendingDialog::new(
            DocumentDialogRequest {
                target: DocumentDialogTarget::OpenFile,
                key: self.document().key(),
            },
            dialog.pick_file_or_folder(),
        ));
    }

    fn load_path(&mut self, path: PathBuf) -> bool {
        let started = Instant::now();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.show_file_error(format!("Could not open {}: {error}", path.display()));
                return false;
            }
        };
        let kind = match crate::document::detect_document(&path, &bytes) {
            Ok(kind) => kind,
            Err(error) => {
                self.show_file_error(error);
                return false;
            }
        };
        let disk_fingerprint = kind.is_editable().then(|| fingerprint(&bytes));
        let source = if kind.is_editable() {
            // DocumentKind::detect already validated UTF-8.
            String::from_utf8(bytes).expect("validated UTF-8 document")
        } else {
            String::new()
        };
        let path = path.canonicalize().unwrap_or(path);
        let reloading_current_document = self
            .document()
            .path()
            .as_ref()
            .is_some_and(|current| same_path(current, &path));
        let keep_designated_preview = self.should_keep_designated_preview(&path);
        let workspace_root_changed = !path.starts_with(&self.workspace_root);
        let preserve_workspace_snapshot = preserve_workspace_snapshot_for_open(
            self.workspace.as_ref().map(WorkspaceTree::root),
            &self.workspace_root,
            &path,
        );
        let retain_tex = reloading_current_document && self.document().config().is_some();
        self.document_mut().replace_loaded_unprojected(
            source,
            path.clone(),
            kind,
            disk_fingerprint,
        );
        if (retain_tex || self.settings.mitex_auto_enable) && kind.is_typst() {
            self.activate_preferred_tex();
        }
        if workspace_root_changed && let Some(parent) = path.parent() {
            self.workspace_root = canonical_or_absolute(&discover_project_root(parent));
        }
        let workspace_root = self.workspace_root.clone();
        self.remember_workspace(&workspace_root);

        self.set_active_autosave_deadline(None);
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.remember_open_document(&path);
        self.clear_preview_for_document(keep_designated_preview);
        self.search.clear();
        if !preserve_workspace_snapshot {
            self.reset_document_services();
        } else if keep_designated_preview {
            if self.tinymist_sync.generation.is_some() {
                self.reopen_tinymist_current_document(&path, kind);
            } else {
                self.restart_tinymist();
            }
        } else if reloading_current_document
            && kind.is_typst()
            && self.tinymist_sync.generation.is_some()
        {
            // Reloading an externally changed document only needs a fresh LSP
            // document. Keep the running Tinymist workspace and preview alive.
            self.reopen_tinymist_current_document(&path, kind);
        } else {
            self.restart_tinymist();
        }

        self.notice = Some(Notice {
            message: completed_action_label("Opened", &self.document().name(), started.elapsed()),
            kind: NoticeKind::Success,
        });
        if kind.preview_only() {
            self.request_asset(path);
        }
        if self.preview_processing_enabled()
            && (reloading_current_document
                || tabs::switch_needs_compile(
                    keep_designated_preview,
                    self.raster_preview_required(),
                    !self.preview.content.pages().is_empty(),
                    matches!(
                        self.preview.status,
                        PreviewStatus::Waiting | PreviewStatus::Compiling
                    ),
                ))
        {
            self.schedule_compile_now();
        } else if !self.typst_preview_available() {
            self.preview.status = PreviewStatus::Ready(Duration::ZERO);
        }
        self.git.request_refresh();
        self.git_editor.request_refresh();
        true
    }

    fn save_document(&mut self, frame: Option<&eframe::Frame>, context: &egui::Context) -> bool {
        if !self.document().kind().is_editable() {
            return true;
        }
        if let Some(path) = self.document().path().clone() {
            self.save_to(path, context)
        } else {
            self.save_as(frame)
        }
    }

    fn save_as(&mut self, frame: Option<&eframe::Frame>) -> bool {
        if !self.document().kind().is_editable() {
            return false;
        }
        if self.document_workflow.has_dialog() {
            return false;
        }
        let typst = self.document().kind().is_typst();
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
        self.document_workflow.start_dialog(PendingDialog::new(
            DocumentDialogRequest {
                target: DocumentDialogTarget::SaveAs { typst },
                key: self.document().key(),
            },
            dialog.save_file(),
        ));
        false
    }

    fn export_pdf(&mut self, frame: Option<&eframe::Frame>) {
        self.choose_pdf_output(frame, PdfWriteIntent::Export);
    }

    fn choose_pdf_output(&mut self, frame: Option<&eframe::Frame>, intent: PdfWriteIntent) {
        if !self.typst_preview_available() && self.document().kind() != DocumentKind::Pdf {
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
            self.document().path().clone()
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
                document_epoch: self.document().epoch(),
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
            if self.document().epoch() == request.document_epoch {
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
        frame: Option<&eframe::Frame>,
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
                "Choose color scheme for light appearance"
            }
            ToolPickerTarget::SublimeTheme { dark_mode: true } => {
                "Choose color scheme for dark appearance"
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
                *edited.color_theme_mut(if requested_dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                }) = ColorThemeChoice::sublime(value);
                let assigned = if requested_dark { "dark" } else { "light" };
                let name = imported.name.as_deref().unwrap_or("Sublime color scheme");
                self.notice = Some(Notice {
                    message: format!("Imported {name} for {assigned} appearance"),
                    kind: NoticeKind::Success,
                });
            }
        }
        self.pending_settings = Some(edited);
        context.request_repaint();
    }

    fn poll_document_dialog(&mut self, context: &egui::Context) {
        self.poll_document_dialog_inner(context);
        self.document_workflow.finish_dispatch();
    }

    fn poll_document_dialog_inner(&mut self, context: &egui::Context) {
        let DialogPoll::Ready { request, selection } =
            self.document_workflow.poll_document_dialog(context)
        else {
            return;
        };
        let target = request.target;
        let key = request.key;
        let Some(file) = selection else {
            self.document_workflow.cancel_continuation();
            self.schedule_autosave_if_needed();
            return;
        };
        let mut path = file.path().to_path_buf();
        if path.is_dir()
            && matches!(
                target,
                DocumentDialogTarget::OpenFile | DocumentDialogTarget::OpenFileInNewWindow
            )
        {
            self.pending_window_requests
                .push_back(EditorWindowRequest::Open(path));
            return;
        }
        match target {
            DocumentDialogTarget::OpenFile => {
                if self.document().epoch() != key.epoch {
                    self.notice = Some(Notice {
                        message: "Open canceled because another document is now active".to_owned(),
                        kind: NoticeKind::Info,
                    });
                } else if self.document().revision() != key.revision {
                    self.request_document_replacement(
                        DeferredDocumentAction::LoadPath(path),
                        "opening the selected document",
                    );
                } else {
                    self.open_tab_path(path, context);
                }
                self.schedule_autosave_if_needed();
            }
            DocumentDialogTarget::OpenFileInNewWindow => {
                self.pending_window_requests
                    .push_back(EditorWindowRequest::Open(path));
            }
            DocumentDialogTarget::OpenFolder => {
                if self.document().epoch() != key.epoch {
                    self.notice = Some(Notice {
                        message: "Open Folder canceled because another document is now active"
                            .to_owned(),
                        kind: NoticeKind::Info,
                    });
                } else if self.document().revision() != key.revision {
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
                if self.document().epoch() != key.epoch {
                    self.document_workflow.cancel_continuation();
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
                self.save_to(path, context);
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

        let typst_output = self.typst_preview_available();
        let output_preview = if typst_output {
            &self.preview
        } else {
            &self.asset_preview
        };
        let requires_new_artifact = pdf_output_requires_new_artifact(
            typst_output,
            self.compilation_paused,
            self.compile_deadline.is_some(),
            output_preview.status,
        );
        let reusable = pdf_artifact_reusable_for_output(
            output_preview.content.artifact_key(),
            output_preview.content.pdf().is_some(),
            self.document().revision(),
            requires_new_artifact,
        );
        self.document_workflow.pending_export = Some(PendingExport {
            path,
            document_epoch: self.document().epoch(),
            intent,
            after_artifact_generation: if typst_output && !reusable {
                self.preview
                    .content
                    .artifact_key()
                    .map(|key| key.generation)
            } else {
                None
            },
        });
        if reusable {
            self.complete_pending_export();
            return;
        }
        self.notice = Some(Notice {
            message: intent.queued_message().to_owned(),
            kind: NoticeKind::Info,
        });
        if self.preview_processing_enabled() {
            let transition = self
                .preview
                .transition(PreviewTransitionEvent::RenderRequested);
            self.apply_preview_transition(transition, None);
        }
    }

    fn complete_pending_export(&mut self) {
        let output_preview = if self.typst_preview_available() {
            &self.preview
        } else {
            &self.asset_preview
        };
        let Some(pdf) = output_preview.content.pdf() else {
            return;
        };
        let document_epoch = self.document().epoch();
        let document_revision = self.document().revision();
        let Some(pending) = take_ready_export(
            &mut self.document_workflow.pending_export,
            document_epoch,
            document_revision,
            output_preview.content.artifact_key(),
        ) else {
            return;
        };
        match atomic_write(&pending.path, pdf) {
            Err(error) => self.show_file_error(error),
            Ok(tiptoptyp::save_transaction::WriteDurability::Uncertain(error)) => {
                self.show_file_error(format!(
                    "Wrote {}, but could not confirm disk durability: {error}",
                    pending.path.display()
                ));
            }
            Ok(tiptoptyp::save_transaction::WriteDurability::Synchronized) => {
                self.notice = Some(Notice {
                    message: format!(
                        "{} {}",
                        pending.intent.completed_verb(),
                        pending.path.display()
                    ),
                    kind: NoticeKind::Success,
                });
            }
        }
        if self.compilation_paused && !self.may_run_compilation() {
            self.compile_deadline = None;
            let _ = self.compiler.pause(self.document().revision());
        }
    }

    fn request_document_replacement(&mut self, action: DeferredDocumentAction, description: &str) {
        if self.document_flow_busy() {
            return;
        }
        if !matches!(action, DeferredDocumentAction::CloseWindow) {
            self.lifecycle.request_resume();
        }
        if matches!(action, DeferredDocumentAction::CloseWindow) {
            self.tabs.approved.clear();
        }
        let key = self.document().key();
        let dirty = self.document().is_dirty() && !tabs::opens_tab(&action);
        let name = self.document().name();
        self.document_workflow
            .queue_replacement(key, dirty, &name, action, description);
    }

    fn present_unsaved_prompt(&mut self, pending: PendingDocumentAction) {
        let key = self.document().key();
        let name = self.document().name();
        self.document_workflow.present_unsaved(key, &name, pending);
    }

    fn execute_pending_document_action(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        self.execute_pending_document_action_inner(context, frame);
        self.document_workflow.finish_dispatch();
        self.advance_tab_window_close(context);
    }

    fn execute_pending_document_action_inner(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        let Some(mut pending) = self.document_workflow.take_action() else {
            return;
        };
        if pending.key.epoch != self.document().epoch() {
            self.document_workflow.cancel_continuation();
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
                self.document_workflow.continue_after_save(*action);
                self.save_document(frame, context);
                return;
            }
            DeferredDocumentAction::ForceSave {
                path,
                key,
                expected_disk_fingerprint,
                observed_disk_fingerprint,
            } => {
                let same_document = key.epoch == self.document().epoch()
                    && self
                        .document()
                        .path()
                        .as_ref()
                        .is_some_and(|current| same_path(current, &path))
                    && self.document().disk_fingerprint() == expected_disk_fingerprint;
                if !same_document {
                    self.document_workflow.cancel_continuation();
                    self.notice = Some(Notice {
                        message: "Overwrite canceled because the document changed".to_owned(),
                        kind: NoticeKind::Error,
                    });
                    self.schedule_autosave_if_needed();
                    return;
                }
                let intent = if key.revision != self.document().revision() {
                    SaveIntent::Explicit
                } else {
                    SaveIntent::ExplicitConfirmed {
                        observed: observed_disk_fingerprint,
                    }
                };
                if let Some(tab) = self.tabs.active_id() {
                    self.submit_save(tab, path, intent, true, context);
                }
                return;
            }
            action => pending.action = action,
        }

        if pending.allow_discard {
            if pending.key.revision != self.document().revision() {
                self.present_unsaved_prompt(pending);
                return;
            }
        } else if self.is_dirty() && !tabs::opens_tab(&pending.action) {
            self.present_unsaved_prompt(pending);
            return;
        }

        match pending.action {
            DeferredDocumentAction::New => {
                self.new_tab(context);
                self.notice = Some(Notice {
                    message: "Created new document".to_owned(),
                    kind: NoticeKind::Success,
                });
            }
            DeferredDocumentAction::OpenFileDialog => self.start_open_dialog(frame),
            DeferredDocumentAction::OpenFolderDialog => self.start_open_folder_dialog(frame),
            DeferredDocumentAction::CloseWindow => {
                self.approve_tab_window_close();
            }
            DeferredDocumentAction::CloseTab => {
                self.finish_close_tab(context);
            }
            DeferredDocumentAction::LoadPath(path) => {
                self.open_tab_path(path, context);
            }
            DeferredDocumentAction::OpenFolder(path) => {
                self.finish_open_folder_selection(path);
            }
            DeferredDocumentAction::FollowFileLink {
                path,
                page,
                source_position,
            } => {
                if self.open_tab_path(path, context) {
                    self.apply_file_link_location(page, source_position);
                }
            }
            DeferredDocumentAction::FollowTinymistLocation { path, selection } => {
                if self.open_tab_path(path, context) {
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
        if !self.lifecycle.allows_document_work() {
            return;
        }
        let root = self.project_root();
        self.workspace = None;
        self.workspace_error = None;
        self.request_workspace_scan(root);
        // Outline, symbols, references, dependencies, and packages are all
        // rooted in the workspace. Clear them synchronously so the Contents
        // sections cannot show paths from the old root while the replacement
        // index is being built.
        self.project_index_job.supersede();
        self.project_index = ProjectIndex::default();
        self.schedule_project_index();
        self.git.request_refresh();
        self.git_editor.request_refresh();
        self.restart_tinymist();
    }

    fn should_keep_designated_preview(&self, _path: &Path) -> bool {
        self.designated_preview_path().is_some()
    }

    /// Switch the active editor document without tearing down the designated
    /// preview document or its native web view. A non-Typst child file has no
    /// current LSP document, but the preview entry remains open and visible in
    /// Split or Preview mode.
    fn reopen_tinymist_current_document(&mut self, path: &Path, kind: DocumentKind) {
        if self.tinymist_sync.generation.is_none() {
            return;
        }
        let input = if kind.is_typst() {
            match crate::tinymist_sync::collect(self.document(), path) {
                Ok(input) => Some(input),
                Err(error) => {
                    self.preview.tinymist_state = ServiceState::Degraded(error);
                    return;
                }
            }
        } else {
            None
        };
        let batch = self.tinymist_sync.switch(
            input,
            self.tabs.len() == 1,
            self.preview.connection.is_ready(),
        );
        if let Err(error) = self.apply_tinymist_sync_batch(batch) {
            self.preview.tinymist_state = ServiceState::Degraded(error);
        }
    }

    fn request_workspace_scan(&mut self, root: PathBuf) {
        if !self.lifecycle.allows_document_work() {
            return;
        }
        if let Err(error) = self.workspace_service.subscribe(&root) {
            self.workspace_error = Some(format!("Could not start workspace scan: {error}"));
        }
    }

    fn poll_workspace_scan(&mut self, context: &egui::Context) {
        while let Some(event) = self.workspace_service.poll() {
            match event {
                WorkspaceEvent::Snapshot {
                    snapshot,
                    scan_serial,
                } => {
                    debug_assert!(scan_serial > 0);
                    let font_files = workspace_snapshot_font_files(&snapshot);
                    let should_rescan_fonts = !self.font_catalog_scan.is_running()
                        && snapshot.root == self.font_catalog_root
                        && !self.font_catalog.workspace_files_match(&font_files);
                    if let Some(workspace) = &mut self.workspace {
                        workspace.apply_shared(snapshot);
                    } else {
                        self.workspace = Some(WorkspaceTree::from_shared(snapshot));
                    }
                    self.workspace_error = None;
                    if should_rescan_fonts {
                        self.request_font_catalog_scan(context);
                    }
                }
                WorkspaceEvent::PathsChanged(paths) => {
                    if self
                        .document()
                        .path()
                        .as_ref()
                        .is_some_and(|active| paths.iter().any(|path| same_path(path, active)))
                    {
                        self.poll_external_file_change(context, true);
                    }
                }
                WorkspaceEvent::VerifyActiveFile => {
                    self.poll_external_file_change(context, false);
                }
                WorkspaceEvent::Error(error) => {
                    self.workspace_error = Some(error);
                }
            }
        }
    }

    fn refresh_workspace(&mut self) {
        self.git.request_refresh();
        self.refresh_workspace_tree();
    }

    fn refresh_workspace_tree(&mut self) {
        if !self.workspace_service.is_subscribed() {
            let root = self
                .workspace
                .as_ref()
                .map(|workspace| workspace.root().to_owned())
                .unwrap_or_else(|| self.project_root());
            self.request_workspace_scan(root);
        } else {
            self.workspace_service.refresh();
        }
    }

    fn tick_workspace(&mut self, context: &egui::Context) {
        self.poll_workspace_scan(context);
    }

    fn poll_external_file_change(&mut self, context: &egui::Context, force_content_check: bool) {
        if self.save_job.is_running() {
            return;
        }
        if self.tabs.is_empty() {
            return;
        }
        // The Tabs scene intentionally uses two in-memory, never-written
        // fixture documents. They are not externally deleted user files.
        if self.snapshot_scene == Some(UiSnapshotScene::Tabs) {
            return;
        }
        let Some(path) = self.document().path().clone() else {
            return;
        };
        if !self.document().kind().is_editable() {
            return;
        }
        let stamp = match external_file_stamp(&path) {
            Ok(stamp) => Some(stamp),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return,
        };
        if !force_content_check && stamp.is_some() && stamp == self.external_file_stamp {
            return;
        }
        self.external_file_stamp = stamp;
        let observed = match fs::read(&path) {
            Ok(contents) => ExternalFileObservation::Present(fingerprint(&contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ExternalFileObservation::Missing
            }
            Err(_) => return,
        };
        if matches!(observed, ExternalFileObservation::Present(fingerprint) if Some(fingerprint) == self.document().disk_fingerprint())
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
        if self.tabs.uses_designated_preview() && self.document().path().is_none() {
            return self.untitled_tab_path(self.tabs.active_id().expect("non-empty tab set"));
        }
        self.tinymist_sync
            .active_backing
            .as_ref()
            .map(|document| document.path().to_owned())
            .or_else(|| self.document().path().clone())
            .unwrap_or_else(|| self.preview_document_path())
    }

    fn restart_tinymist(&mut self) {
        let transition = self.preview.transition(PreviewTransitionEvent::Restart {
            preserve_surface: false,
        });
        self.apply_preview_transition(transition, None);
    }

    fn restart_tinymist_preserving_preview(&mut self) {
        let transition = self.preview.transition(PreviewTransitionEvent::Restart {
            preserve_surface: true,
        });
        self.apply_preview_transition(transition, None);
    }

    fn restart_tinymist_for_preview_entry(&mut self) {
        let transition = self
            .preview
            .transition(PreviewTransitionEvent::PreviewEntryChanged);
        self.apply_preview_transition(transition, None);
    }

    fn handle_tinymist_failure(&mut self, event: &TinymistEvent, context: &egui::Context) -> bool {
        let transition = self
            .preview
            .transition(PreviewTransitionEvent::Failure(event, Instant::now()));
        if !transition.handled {
            return false;
        }
        self.apply_preview_transition(transition, Some(context));
        true
    }

    fn tick_tinymist_recovery(&mut self, context: &egui::Context) {
        let transition = self
            .preview
            .transition(PreviewTransitionEvent::RecoveryTick(Instant::now()));
        self.apply_preview_transition(transition, Some(context));
    }

    fn apply_preview_transition(
        &mut self,
        transition: PreviewTransition,
        context: Option<&egui::Context>,
    ) {
        for effect in transition.effects {
            match effect {
                PreviewEffect::StopAttempt(generation) => {
                    // The subsequent Stopped event belongs to this attempt;
                    // Recovery rejects it as a duplicate failure.
                    let _ = self.tinymist.stop_workspace(generation);
                }
                PreviewEffect::StopService => self.stop_tinymist_session_io(),
                PreviewEffect::RestartService { preserve_surface } => {
                    self.restart_tinymist_with_handoff(preserve_surface)
                }
                PreviewEffect::SetRefresh {
                    generation,
                    refresh,
                } => {
                    if let Err(error) = self.tinymist.set_preview_refresh(generation, refresh) {
                        self.preview.tinymist_state = ServiceState::Degraded(error.to_string());
                    }
                }
                PreviewEffect::ScheduleRaster => self.schedule_compile_now_io(),
                PreviewEffect::RepaintAfter(delay) => {
                    if let Some(context) = context {
                        context.request_repaint_after(delay);
                    }
                }
                PreviewEffect::DiscardLanguageRequests => {
                    self.manual_format_revision = None;
                    self.format_request_key = None;
                    self.format_when_tinymist_ready = None;
                    self.editor_completion = None;
                }
            }
        }
    }

    fn stop_tinymist_session(&mut self) {
        let transition = self.preview.transition(PreviewTransitionEvent::Stop);
        self.apply_preview_transition(transition, None);
    }

    fn stop_tinymist_session_io(&mut self) {
        for effect in self.tinymist_sync.stop_effects() {
            if let crate::tinymist_sync::Effect::Close { generation, uri } = effect {
                let _ = self.tinymist.did_close(generation, uri);
            }
        }
        if let Some(generation) = self.tinymist_sync.generation {
            let _ = self.tinymist.stop_workspace(generation);
        }
        // Keep the private backing alive through didClose, then clean it.
        self.tinymist_sync.finish_stop();
    }

    fn restart_tinymist_with_handoff(&mut self, preserve_preview: bool) {
        if !self.lifecycle.allows_document_work() || self.tabs.is_empty() {
            return;
        }
        self.manual_format_revision = None;
        self.format_request_key = None;
        self.editor_completion = None;
        let start_preview = self.tinymist_session_requested();
        self.preview.tinymist_preview_enabled = start_preview;
        self.stop_tinymist_session_io();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let has_webview = self.webview.is_some();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let has_webview = false;
        let retain_preview_surface = retain_preview_surface_for_restart(
            preserve_preview,
            start_preview,
            self.preview.connection.endpoint().is_some(),
            has_webview,
        );
        self.preview.connection.suspend(retain_preview_surface);
        self.preview.tinymist_diagnostics.clear();
        self.mark_diagnostics_changed();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.webview_reload_pending = false;
            if !retain_preview_surface {
                self.webview = None;
                self.webview_applied = None;
                self.webview_url = None;
                self.webview_navigation = None;
            }
        }
        if !self.document().kind().is_typst() && self.designated_preview_path().is_none() {
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
        if let Err(error) = self.prepare_tab_backings() {
            self.preview.tinymist_state = ServiceState::Degraded(error);
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
        let mut config = TinymistConfig::new(self.tab_preview_root())
            .with_executable(self.tinymist_tool.program.clone())
            .with_font_paths(self.font_catalog.workspace_directories());
        if let Some(entry_path) = self.designated_preview_path() {
            config = config.with_entry_path(entry_path);
        }
        config.start_preview = start_preview;
        config.preview.invert_colors =
            tinymist_invert_colors(self.settings.document_theme, self.preview.dark);
        config.preview.refresh = tinymist_preview_refresh(self.compilation_paused);
        let source = match self.canonical_document_source() {
            Ok(source) => source,
            Err(error) => {
                self.preview.tinymist_state = ServiceState::Degraded(error);
                return;
            }
        };
        match self.tinymist.start_workspace(config) {
            Ok(generation) => {
                self.preview.recovery.started(generation);
                let version = revision_as_i32(self.document().revision());
                let current_document = if self.document().kind().is_typst() {
                    if let Some(path) = self.document().path().as_deref() {
                        TextDocument::from_path(path, version, source.clone())
                            .map_err(|error| error.to_string())
                    } else if self.tabs.uses_designated_preview() {
                        TextDocument::from_path(
                            &self.untitled_tab_path(
                                self.tabs.active_id().expect("non-empty tab set"),
                            ),
                            version,
                            source.clone(),
                        )
                        .map_err(|error| error.to_string())
                    } else {
                        let source_dir = self
                            .current_directory()
                            .unwrap_or_else(|| self.project_root());
                        match UnsavedTextDocument::create(
                            self.project_root(),
                            source_dir,
                            self.document_name(),
                            &source,
                        ) {
                            Ok(document) => {
                                let text_document = document.text_document(version, source.clone());
                                self.tinymist_sync.active_backing = Some(document);
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
                    if !self.document().kind().is_typst() || self.current_is_preview_document() {
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

                self.preview.connection.start(generation);
                let preview_key = self
                    .tab_preview_document()
                    .map_or_else(|| self.document().key(), |document| document.key());
                let preview_input = crate::tinymist_sync::VersionedInput {
                    key: preview_key,
                    uri: preview_document.uri,
                    version: preview_document.version,
                    source: preview_document.text,
                };
                let batch =
                    self.tinymist_sync
                        .begin(generation, current_document.uri, preview_input);
                // The first document opened determines startDefaultPreview.
                // Imported/current subfiles are opened after Initialized.
                if let Err(error) = self.apply_tinymist_sync_batch(batch) {
                    self.preview.tinymist_state = ServiceState::Failed(error);
                }
            }
            Err(error) => {
                self.preview.connection.clear_endpoint();
                self.preview.tinymist_state = ServiceState::Failed(error.to_string());
                self.preview.webview_state =
                    ServiceState::Failed("Could not restart the pinned preview".to_owned());
            }
        }
    }

    fn sync_tinymist_change(&mut self) -> Result<(), String> {
        if !self.document().kind().is_typst() {
            return Ok(());
        }
        if self.tinymist_sync.active_backing.is_none()
            && (!self.tinymist_sync.current_open
                || self.tinymist_sync.generation.is_none()
                || self.tinymist_sync.current_uri.is_none())
        {
            // No source consumer: avoid allocating/copying the document on
            // every keystroke while the language server is unavailable.
            return Ok(());
        }
        let path = self.tinymist_document_path();
        let input = crate::tinymist_sync::collect(self.document(), &path)?;
        let batch = self.tinymist_sync.edit(self.tabs.active_id(), input);
        self.apply_tinymist_sync_batch(batch)
    }

    fn apply_tinymist_sync_batch(
        &mut self,
        mut batch: crate::tinymist_sync::Batch,
    ) -> Result<(), String> {
        let mut source_available = true;
        for effect in batch.effects {
            use crate::tinymist_sync::Effect;
            match effect {
                Effect::UpdateBacking(target) => {
                    if let Some(backing) = self.tinymist_sync.backing(target) {
                        backing
                            .update_backing_source(&batch.source)
                            .map_err(|error| error.to_string())?;
                    }
                }
                Effect::Open {
                    generation,
                    uri,
                    version,
                } => {
                    debug_assert!(
                        source_available,
                        "one source-bearing service effect per batch"
                    );
                    source_available = false;
                    self.tinymist
                        .did_open(
                            generation,
                            TextDocument::typst(
                                uri.clone(),
                                version,
                                std::mem::take(&mut batch.source),
                            ),
                        )
                        .map_err(|error| error.to_string())?;
                    self.tinymist_sync.confirm_open(&uri);
                }
                Effect::Change {
                    generation,
                    uri,
                    version,
                } => {
                    debug_assert!(
                        source_available,
                        "one source-bearing service effect per batch"
                    );
                    source_available = false;
                    self.tinymist
                        .did_change(generation, uri, version, std::mem::take(&mut batch.source))
                        .map_err(|error| error.to_string())?;
                }
                Effect::Close { generation, uri } => self
                    .tinymist
                    .did_close(generation, uri)
                    .map_err(|error| error.to_string())?,
            }
        }
        Ok(())
    }

    fn receive_tinymist_events(&mut self, context: &egui::Context) {
        while let Some(event) = self.tinymist.try_recv() {
            if self.handle_tinymist_failure(&event, context) {
                continue;
            }
            match event {
                TinymistEvent::CompileStatus {
                    generation,
                    path,
                    status,
                    received,
                } => {
                    if self.tinymist_sync.generation == Some(generation)
                        && self.preview.recovery.accepts(generation)
                        && self.preview.tinymist_preview_enabled
                        && self.may_run_compilation()
                        && tinymist_status_matches_entry(
                            &path,
                            &self
                                .designated_preview_path()
                                .unwrap_or_else(|| self.tinymist_document_path()),
                            &self.project_root(),
                        )
                    {
                        self.preview
                            .compile_status(generation, path, status, received);
                    }
                }
                TinymistEvent::Starting { .. } => {
                    self.editor_completion = None;
                    self.preview.tinymist_state =
                        ServiceState::Starting("Launching Tinymist LSP".to_owned());
                }
                TinymistEvent::Initialized { generation } => {
                    if !self.preview.recovery.accepts(generation) {
                        continue;
                    }
                    if !self.preview.connection.initialized(generation) {
                        continue;
                    }
                    if !self.preview.tinymist_preview_enabled {
                        self.preview.recovery.recovered(generation);
                    }
                    self.preview.tinymist_state = if self.preview.tinymist_preview_enabled {
                        ServiceState::Starting("Starting Tinymist preview server".to_owned())
                    } else {
                        ServiceState::Ready("Tinymist LSP is ready".to_owned())
                    };
                    if !self.tinymist_sync.current_open
                        && self.tinymist_sync.generation == Some(generation)
                    {
                        let path = self.tinymist_document_path();
                        let input = match crate::tinymist_sync::collect(self.document(), &path) {
                            Ok(input) => input,
                            Err(error) => {
                                self.preview.tinymist_state = ServiceState::Degraded(error);
                                continue;
                            }
                        };
                        let batch = self.tinymist_sync.switch(Some(input), false, true);
                        if let Err(error) = self.apply_tinymist_sync_batch(batch) {
                            self.preview.tinymist_state = ServiceState::Degraded(error);
                        }
                    }
                    self.sync_parked_tinymist();
                    let document_key = self.document().key();
                    if take_ready_format_handoff(
                        &mut self.format_when_tinymist_ready,
                        document_key,
                        self.tinymist_sync.generation == Some(generation)
                            && self.tinymist_sync.current_open,
                    ) {
                        self.request_format_after_manual_save();
                    }
                }
                TinymistEvent::PreviewReady { generation, url } => {
                    if !self.preview.recovery.recovered(generation) {
                        continue;
                    }
                    self.preview.tinymist_state =
                        ServiceState::Ready("LSP and preview server are ready".to_owned());
                    if self.preview.tinymist_preview_enabled {
                        let Ok(endpoint) = url::Url::parse(&url) else {
                            self.preview.webview_state =
                                ServiceState::Failed("Invalid preview endpoint".to_owned());
                            continue;
                        };
                        if !self.preview.connection.connect(generation, endpoint) {
                            continue;
                        }
                        if matches!(self.preview.status, PreviewStatus::Waiting) {
                            self.preview.status = PreviewStatus::Ready(Duration::ZERO);
                        }
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
                TinymistEvent::ShowDocument {
                    generation,
                    uri,
                    selection,
                    ..
                } => {
                    if self.tinymist_sync.generation == Some(generation) {
                        self.follow_tinymist_location(&uri, selection.as_ref());
                    }
                }
                TinymistEvent::PublishDiagnostics {
                    generation,
                    uri,
                    version,
                    diagnostics,
                    ..
                } => {
                    if self.tinymist_sync.generation == Some(generation) {
                        self.receive_tinymist_diagnostics(&uri, version, diagnostics);
                    }
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
                    generation,
                    uri,
                    version,
                    request_token,
                    contents,
                    ..
                } => {
                    let current_key = self.document().key();
                    let request_key = self.editor_hover.as_ref().and_then(|hover| {
                        (hover.request_token == request_token
                            && hover.version == version
                            && hover.uri == uri)
                            .then_some(hover.key)
                    });
                    let current = request_key.is_some_and(|key| {
                        self.tinymist_sync.accepts_reply(
                            crate::tinymist_sync::ReplyIdentity {
                                generation,
                                uri: &uri,
                                version,
                                key,
                            },
                            current_key,
                        )
                    });
                    if current
                        && let Some(hover) = &mut self.editor_hover
                        && hover.request_token == request_token
                        && hover.version == version
                        && hover.uri == uri
                    {
                        hover.detail = contents.map(Arc::from);
                    }
                }
                TinymistEvent::Completed {
                    generation,
                    uri,
                    version,
                    request_token,
                    is_incomplete,
                    items,
                } => self.receive_editor_completions(
                    EditorCompletionResponse {
                        generation,
                        uri,
                        version,
                        request_token,
                        is_incomplete,
                        items,
                    },
                    context,
                ),
                TinymistEvent::CompletionFailed {
                    generation,
                    uri,
                    version,
                    request_token,
                    message,
                } => {
                    let matches = self.editor_completion.as_ref().is_some_and(|completion| {
                        completion_response_matches(
                            completion,
                            generation,
                            &uri,
                            version,
                            request_token,
                            self.tinymist_sync.generation,
                            self.tinymist_sync.current_uri.as_deref(),
                            revision_as_i32(self.document().revision()),
                        )
                    });
                    if matches {
                        let explicit = self
                            .editor_completion
                            .as_ref()
                            .is_some_and(|completion| completion.explicit);
                        self.editor_completion = None;
                        if explicit {
                            self.notice = Some(Notice {
                                message: format!("Completion failed: {message}"),
                                kind: NoticeKind::Error,
                            });
                        }
                    }
                }
                TinymistEvent::Error { stage, message, .. } => {
                    if stage == "formatting" {
                        self.manual_format_revision = None;
                        self.format_request_key = None;
                        self.notice = Some(Notice {
                            message: format!("Formatting failed: {message}"),
                            kind: NoticeKind::Error,
                        });
                        continue;
                    }
                    self.preview.tinymist_state =
                        ServiceState::Degraded(format!("{stage}: {message}"));
                    if self.preview_processing_enabled() {
                        self.schedule_compile_now();
                    }
                }
                _ => {}
            }
        }
    }

    fn receive_web_links(&mut self) {
        if let Some(receiver) = &self.browser_launch {
            match receiver.try_recv() {
                Ok(result) => {
                    self.browser_launch = None;
                    self.notice = Some(match result {
                        Ok(target) => external_link_opened_notice(&target),
                        Err(message) => Notice {
                            message,
                            kind: NoticeKind::Error,
                        },
                    });
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.browser_launch = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut handled = std::collections::HashSet::new();
        while let Ok(target) = self.web_link_receiver.try_recv() {
            if handled.insert(target.clone()) {
                self.handle_web_link(&target);
            }
        }
    }

    fn handle_web_link(&mut self, target: &str) {
        self.follow_preview_link(target);
    }

    fn follow_preview_link(&mut self, target: &str) {
        let directory = if self.typst_preview_available() {
            Some(self.preview_source_directory())
        } else {
            self.current_directory()
        };
        self.follow_preview_link_from(target, directory);
    }

    fn follow_preview_link_from(&mut self, target: &str, directory: Option<PathBuf>) {
        if let Some(page) = internal_pdf_page_target(target) {
            self.preview.requested_page =
                Some(page.min(self.preview.content.pages().len().saturating_sub(1)));
            return;
        }

        if let Some(target) = normalize_browser_link_target(target) {
            self.open_external_link(&target);
            return;
        }

        let url = match url::Url::parse(target) {
            Ok(url) => url,
            Err(_) => {
                let Some(directory) = directory else {
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
            self.open_external_link(url.as_str());
        } else {
            self.notice = Some(Notice {
                message: format!("Unsupported link scheme: {}", url.scheme()),
                kind: NoticeKind::Error,
            });
        }
    }

    fn open_external_link(&mut self, target: &str) {
        if self.browser_launch.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let target = target.to_owned();
        let repaint = self.browser_repaint.clone();
        match std::thread::Builder::new()
            .name("tiptoptyp-browser-launch".into())
            .spawn(move || {
                let result = open_in_system_browser(&target).map(|()| target);
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(_) => {
                self.browser_launch = Some(receiver);
                self.notice = Some(Notice {
                    message: "Opening link in your browser…".into(),
                    kind: NoticeKind::Info,
                });
            }
            Err(error) => {
                self.notice = Some(Notice {
                    message: format!("Could not start browser launcher: {error}"),
                    kind: NoticeKind::Error,
                });
            }
        }
    }

    fn follow_file_link(&mut self, url: &url::Url) {
        let page = pdf_page_from_url(url);
        let source_position = source_position_from_url(url);
        let Ok(path) = url.to_file_path() else {
            return;
        };
        if !crate::document::supports_path(&path) && path.extension().is_some() {
            self.notice = Some(Notice {
                message: format!("Unsupported linked file: {}", path.display()),
                kind: NoticeKind::Error,
            });
            return;
        }
        self.navigate_file_location(path, page, source_position, "following a document link");
    }

    fn jump_source_to_preview(&mut self, char_index: usize) {
        if !self.document().kind().is_typst() || !self.interactive_preview_active() {
            return;
        }
        let Some(generation) = self.tinymist_sync.generation else {
            return;
        };
        let position = if self.document().config().is_none() {
            Some(scalar_position_at(
                self.document().source(),
                ScalarOffset::new(char_index),
            ))
        } else {
            self.document()
                .canonical_snapshot()
                .ok()
                .and_then(|snapshot| {
                    snapshot.canonical_preview_position(ScalarOffset::new(char_index))
                })
        };
        let Some((line, character)) = position else {
            return;
        };
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
        !self.tabs.is_empty()
            && preview_visible_for(
                self.document().kind(),
                self.view_mode,
                self.designated_preview_path().is_some(),
            )
    }

    fn preview_status_snapshot(&self) -> PreviewStatusSnapshot<'_> {
        self.preview.status_snapshot(
            self.typst_preview_available(),
            self.preview_visible(),
            cfg!(any(target_os = "macos", target_os = "windows")),
            self.document().revision(),
        )
    }

    fn interactive_preview_requested(&self) -> bool {
        self.preview_status_snapshot().interactive_requested
    }

    fn tinymist_session_requested(&self) -> bool {
        self.preview_status_snapshot().interactive_requested
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
        self.webview_applied = None;
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
            let transition = self
                .preview
                .transition(PreviewTransitionEvent::RenderRequested);
            self.apply_preview_transition(transition, None);
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
            let transition = self
                .preview
                .transition(PreviewTransitionEvent::RenderRequested);
            self.apply_preview_transition(transition, None);
        }
    }

    fn receive_tinymist_diagnostics(
        &mut self,
        uri: &str,
        version: Option<i32>,
        diagnostics: Vec<TinymistDiagnostic>,
    ) {
        if self.tinymist_sync.current_uri.as_deref() == Some(uri)
            && version.is_some_and(|version| version != revision_as_i32(self.document().revision()))
        {
            return;
        }
        let mapped = if self.document().config().is_some()
            && self.tinymist_sync.current_uri.as_deref() == Some(uri)
        {
            if version != Some(revision_as_i32(self.document().revision())) {
                return;
            }
            let Ok(snapshot) = self.document().canonical_snapshot() else {
                return;
            };
            Some(snapshot)
        } else {
            None
        };
        let source = if self.tinymist_sync.preview_uri.as_deref() == Some(uri) {
            DiagnosticSource::Main
        } else {
            url::Url::parse(uri)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .map_or(DiagnosticSource::Global, DiagnosticSource::File)
        };
        let mut converted = diagnostics
            .into_iter()
            .map(|diagnostic| {
                let editor_range = mapped
                    .as_ref()
                    .and_then(|snapshot| snapshot.editor_range(diagnostic.range));
                let mut diagnostic = tinymist_diagnostic(diagnostic, source.clone());
                if mapped.is_some() {
                    diagnostic.location = editor_range.map(|range| {
                        let (line, column) =
                            line_column_at_char(self.document().source(), range.start);
                        crate::diagnostics::DiagnosticLocation { line, column }
                    });
                }
                diagnostic
            })
            .collect();
        normalize_diagnostics(&mut converted);
        self.preview.tinymist_diagnostics = converted;
        self.mark_diagnostics_changed();
    }

    fn interactive_preview_active(&self) -> bool {
        self.preview_status_snapshot().native_ready
    }

    fn preview_fallback_reason(&self) -> Option<String> {
        self.preview_status_snapshot().fallback_reason()
    }

    fn non_preview_fallback_details(&self, system_theme: Option<egui::Theme>) -> Vec<String> {
        non_preview_fallback_details_for(
            &self.settings,
            system_theme,
            &self.typst_tool,
            &self.tinymist_tool,
        )
    }

    fn show_toolbar(&mut self, ui: &mut egui::Ui, frame: Option<&eframe::Frame>) {
        let shortcuts = self.settings.effective_shortcuts();
        #[cfg(target_os = "macos")]
        {
            // The toolbar occupies the native title-bar row. Controls start to
            // the right of the traffic lights, while empty toolbar space stays
            // draggable. A tab title is also a drag target; skip the broad
            // window target while the pointer is over a tab or a tab drag is
            // already in progress, otherwise macOS starts moving the window.
            let pointer_claimed_by_tab = ui.ctx().input(|input| {
                self.tabs
                    .claims_window_drag(input.pointer.latest_pos(), input.pointer.primary_down())
            });
            // AppKit can move a full-size-content window directly, without
            // any egui StartDrag command. Disable that path on hover, before
            // mouse-down, and retain the guard throughout a tab gesture.
            let focused = ui
                .ctx()
                .input(|input| input.viewport().focused == Some(true));
            self.tabs.suppress_native_drag(
                self.native_window_parent.as_ref(),
                pointer_claimed_by_tab && focused,
            );
            if !pointer_claimed_by_tab {
                let drag = ui.interact(
                    ui.max_rect(),
                    ui.id().with("window-title-drag"),
                    Sense::drag(),
                );
                if drag.drag_started() {
                    self.tabs.trace_native_drag(ui.ctx());
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
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
                    .then(|| frame.and_then(|frame| frame.window_handle().ok()))
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
            let view_mode_enabled = view_mode_controls_enabled(self.document().kind());
            if compact {
                theme::apply_dense_toolbar_spacing(ui);
            }

            if self.snapshot_scene == Some(UiSnapshotScene::WindowColor) {
                crate::window_logo::snapshot_fixture(ui.ctx());
            }
            crate::window_logo::show(ui, &self.captures);
            if self.settings.titlebar_menus {
                self.show_file_menu(ui);
                self.show_edit_menu(ui);
                self.show_view_menu(ui);
                ui.separator();
            }

            let tex_available = self.tex_mode_available();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if native_hover_text(
                    ui.selectable_label(
                        self.problems_visible,
                        if compact { "!" } else { "Problems" },
                    ),
                    shortcut_tooltip(
                        "Toggle compiler diagnostics",
                        &shortcuts,
                        ShortcutAction::Problems,
                    ),
                )
                .clicked()
                {
                    self.problems_visible = !self.problems_visible;
                }
                ui.add_enabled_ui(view_mode_enabled, |ui| {
                    native_hover_text(
                        ui.selectable_value(
                            &mut self.view_mode,
                            ViewMode::Preview,
                            if compact { "P" } else { "Preview" },
                        ),
                        "Preview (Typst documents only)",
                    );
                    native_hover_text(
                        ui.selectable_value(
                            &mut self.view_mode,
                            ViewMode::Split,
                            if compact { "S" } else { "Split" },
                        ),
                        "Split (Typst documents only)",
                    );
                    native_hover_text(
                        ui.selectable_value(
                            &mut self.view_mode,
                            ViewMode::Code,
                            if compact { "C" } else { "Code" },
                        ),
                        "Code (Typst documents only)",
                    );
                });
                let explorer_label = if compact { "Files" } else { "Explorer" };
                if native_hover_text(
                    ui.selectable_label(self.explorer.panel_visible(), explorer_label),
                    "Toggle the file explorer",
                )
                .clicked()
                {
                    self.explorer.toggle();
                    ui.ctx().request_repaint();
                }
                if native_hover_text(
                    ui.selectable_label(false, if compact { "Set" } else { "Settings" }),
                    "Open Settings in a separate window",
                )
                .clicked()
                {
                    self.toggle_settings();
                }

                ui.separator();
                if native_hover_text(
                    ui.add_enabled(self.typst_preview_available(), egui::Button::new("Compile")),
                    shortcut_tooltip(
                        "Compile PDF beside the Typst source",
                        &shortcuts,
                        ShortcutAction::Compile,
                    ),
                )
                .clicked()
                {
                    self.compile_pdf(frame);
                }

                let (pause_label, pause_hint) = compilation_toggle_copy(self.compilation_paused);
                if native_hover_text(
                    ui.add_enabled(
                        self.typst_preview_available(),
                        egui::Button::new(pause_label).selected(self.compilation_paused),
                    ),
                    shortcut_tooltip(pause_hint, &shortcuts, ShortcutAction::ToggleCompilation),
                )
                .clicked()
                {
                    self.toggle_compilation_paused();
                }
                if native_hover_text(
                    ui.add_enabled(!self.tabs.is_empty(), egui::Button::new("Find")),
                    shortcut_tooltip("Find", &shortcuts, ShortcutAction::Find),
                )
                .clicked()
                {
                    self.toggle_find();
                }

                if tex_available {
                    let active = self.document().config().is_some();
                    let response = ui.selectable_label(active, "miTeX");
                    if response.clicked() {
                        self.set_tex_mode(!active, ui.ctx());
                    }
                    native_hover_text(
                        response,
                        if active {
                            "miTeX is on: dollar math saves as MiTeX calls. Click to turn off."
                        } else {
                            "Use TeX inside dollar math. Saves standard MiTeX calls."
                        },
                    );
                }
                // Lay out the title last so it gets precisely the space left
                // between the menus and the right-aligned control group.
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    self.show_document_title(ui, frame, compact);
                });
            });
        });
    }

    fn show_document_title(
        &mut self,
        ui: &mut egui::Ui,
        frame: Option<&eframe::Frame>,
        _compact: bool,
    ) {
        self.show_tabs(ui, frame);
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
        self.document().history_availability()
    }

    fn undo_editor(&mut self, context: &egui::Context, redo: bool) {
        if !self.document().kind().is_editable() {
            return;
        }
        let current = self.editor_snapshot(context);
        let previous_key = self.document().key();
        let next = self.document_mut().history_step(redo, current.cursor);
        let Some(next) = next else {
            return;
        };
        let changed = previous_key != self.document().key();
        self.store_editor_cursor(context, next.cursor);
        self.document_mut().set_history_reset(false);
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
            self.document().source().chars().count(),
        )));
        state.store(context, source_editor_id(context));
    }

    fn selected_editor_chars(&self, context: &egui::Context) -> Option<Range<usize>> {
        let state = egui::text_edit::TextEditState::load(context, source_editor_id(context))?;
        let range = state.cursor.char_range()?.as_sorted_char_range();
        let len = self.document().source().chars().count();
        let range = range.start.0.min(len)..range.end.0.min(len);
        (range.start != range.end).then_some(range)
    }

    fn copy_editor_selection(&mut self, context: &egui::Context) {
        let Some(range) = self.selected_editor_chars(context) else {
            return;
        };
        self.prepare_editor_source_data();
        let bytes = self.editor_data.char_range_to_byte(range);
        context.copy_text(self.document().source()[bytes].to_owned());
    }

    fn cut_editor_selection(&mut self, context: &egui::Context) {
        if !self.document().kind().is_editable() {
            return;
        }
        let Some(range) = self.selected_editor_chars(context) else {
            return;
        };
        let cursor = self.editor_snapshot(context).cursor;
        let bytes = self.editor_data.char_range_to_byte(range.clone());
        context.copy_text(self.document().source()[bytes.clone()].to_owned());
        self.document_mut()
            .edit(cursor, |source| source.replace_range(bytes, ""));
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
        if self.document().kind().is_editable() {
            if self.document().kind().is_typst() && !self.view_mode.shows_code() {
                self.view_mode = ViewMode::Split;
            }
            let editor_id = source_editor_id(context);
            context.memory_mut(|memory| memory.request_focus(editor_id));
            self.request_widget_paste(context, context.viewport_id());
        }
    }

    fn select_all_editor(&self, context: &egui::Context) {
        let editor_id = source_editor_id(context);
        if let Some(mut state) = egui::text_edit::TextEditState::load(context, editor_id) {
            state.cursor.set_char_range(Some(CCursorRange::two(
                CCursor::new(0),
                CCursor::new(self.document().source().chars().count()),
            )));
            state.store(context, editor_id);
            context.memory_mut(|memory| memory.request_focus(editor_id));
        }
    }

    fn toggle_comments(&mut self, context: &egui::Context) {
        if !self.document().kind().is_editable() {
            return;
        }
        let editor_id = source_editor_id(context);
        let state = egui::text_edit::TextEditState::load(context, editor_id);
        let cursor = state
            .as_ref()
            .and_then(|state| state.cursor.char_range())
            .map_or(0, |range| range.primary.index.0)
            .min(self.document().source().chars().count());
        let selection = state
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.as_sorted_char_range())
            .map(|range| {
                range.start.0.min(self.document().source().chars().count())
                    ..range.end.0.min(self.document().source().chars().count())
            })
            .filter(|range| range.start != range.end);
        let (source, mapped_range) = toggle_line_comments(
            self.document().source(),
            selection.clone().unwrap_or(cursor..cursor),
            "// ",
        );
        if source == *self.document().source() {
            return;
        }
        let cursor = self.editor_snapshot(context).cursor;
        self.document_mut().edit(cursor, |buffer| *buffer = source);
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
        if !self.document().kind().is_typst() {
            self.manual_format_revision = None;
            self.format_request_key = None;
            return;
        }
        if !self.preview.connection.is_ready() {
            self.manual_format_revision = None;
            self.format_request_key = None;
            self.notice = Some(Notice {
                message: "Tinymist is still starting; try formatting again in a moment".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        }
        let (Some(generation), Some(uri)) = (
            self.tinymist_sync.generation,
            self.tinymist_sync.current_uri.clone(),
        ) else {
            self.manual_format_revision = None;
            self.format_request_key = None;
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
            self.format_request_key = None;
            self.notice = Some(Notice {
                message: format!("Could not prepare the document for formatting: {error}"),
                kind: NoticeKind::Error,
            });
            return;
        }
        match self.tinymist.format_document(
            generation,
            uri,
            revision_as_i32(self.document().revision()),
        ) {
            Ok(()) => {
                self.format_request_key = Some(self.document().key());
                self.notice = Some(Notice {
                    message: "Formatting with Tinymist…".to_owned(),
                    kind: NoticeKind::Info,
                });
            }
            Err(error) => {
                self.manual_format_revision = None;
                self.format_request_key = None;
                self.notice = Some(Notice {
                    message: format!("Could not request formatting: {error}"),
                    kind: NoticeKind::Error,
                });
            }
        }
    }

    fn request_format_after_manual_save(&mut self) {
        if self.document().kind().is_typst() {
            self.manual_format_revision = Some(self.document().revision());
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
        let Some(request_key) = self.format_request_key else {
            return;
        };
        if !self.tinymist_sync.accepts_reply(
            crate::tinymist_sync::ReplyIdentity {
                generation,
                uri,
                version,
                key: request_key,
            },
            self.document().key(),
        ) {
            return;
        }
        self.format_request_key = None;
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
        let cursor = if self.document().reset_editor_history {
            CCursorRange::one(CCursor::new(0))
        } else {
            self.editor_snapshot(context).cursor
        };
        let applied = match self.document().prepare_canonical_edits(
            self.document().key(),
            &edits,
            ([cursor.primary.index.0, cursor.secondary.index.0]).map(ScalarOffset::new),
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
            primary: CCursor::new(applied.mapped_offsets[0].get()),
            secondary: CCursor::new(applied.mapped_offsets[1].get()),
            h_pos: cursor.h_pos,
        };
        let formatted = applied.text;
        if formatted == *self.document().source() {
            self.manual_format_revision = None;
            self.notice = Some(Notice {
                message: "Document is already formatted".to_owned(),
                kind: NoticeKind::Success,
            });
            return;
        }
        self.pending_editor_selection = None;
        let save_after_format = self.manual_format_revision == Some(self.document().revision());
        self.document_mut()
            .edit(cursor, |source| *source = formatted);
        self.store_editor_cursor(context, mapped_cursor);
        self.document_mut().set_history_reset(false);
        self.search.clear();
        self.manual_format_revision = None;
        self.mark_edited();
        let saved_after_format = if save_after_format {
            if let Some(path) = self.document().path().clone() {
                if !self.save_to_with_intent(path, SaveIntent::Explicit, context) {
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
                "Formatted with Tinymist; saving…".to_owned()
            } else {
                "Formatted with Tinymist".to_owned()
            },
            kind: NoticeKind::Success,
        });
    }

    fn begin_table_editor(&mut self, table: EditableTable) {
        let Some(original_call) =
            char_range_slice(self.document().source(), table.source_range.clone())
        else {
            self.notice = Some(Notice {
                message: "The table changed before it could be opened".to_owned(),
                kind: NoticeKind::Info,
            });
            return;
        };
        self.table_editor = Some(TableEditorDialog {
            table,
            original_call: original_call.to_owned(),
            document_key: self.document().key(),
            focus_first_cell: true,
            error: None,
        });
        self.table_editor_had_focus = false;
        self.table_editor_suspended = false;
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
            .document()
            .path()
            .as_ref()
            .is_some_and(|current| same_path(current, &old_path));
        let renames_preview_document = self
            .designated_preview_path()
            .is_some_and(|preview| same_path(&preview, &old_path));
        let preserve_designated_preview = renames_current_document
            && !renames_preview_document
            && self.should_keep_designated_preview(&new_path);
        let reload_binary_document =
            renames_current_document && self.document().kind().preview_only();
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
        let renamed_kind =
            crate::document::detect_document(&new_path, self.document().source().as_bytes())
                .ok()
                .filter(|kind| kind.is_editable())
                .unwrap_or(DocumentKind::Text);
        if renames_current_document && let Err(error) = self.document().can_rename(renamed_kind) {
            self.show_file_error(error.to_string());
            return;
        }
        if let Err(error) = self.preflight_parked_rename(&old_path, &new_path) {
            self.show_file_error(error);
            return;
        }
        if let Err(error) = fs::rename(&old_path, &new_path) {
            self.show_file_error(format!("Could not rename {}: {error}", old_path.display()));
            return;
        }
        let new_path = new_path.canonicalize().unwrap_or(new_path);
        self.rename_parked_tab(&old_path, &new_path);
        if reload_binary_document {
            // Reload from the renamed file so detection sees the real binary
            // bytes. Using `self.document().source()` here would pass an empty buffer for
            // PDFs/images and could incorrectly turn them into editable text.
            self.load_path(new_path.clone());
        } else if renames_current_document {
            self.document_mut()
                .rename(new_path.clone(), renamed_kind)
                .expect("rename preflight above");
            self.remember_open_document(&new_path);
            if preserve_designated_preview {
                self.reopen_tinymist_current_document(&new_path, self.document().kind());
                self.refresh_workspace();
            } else {
                self.reset_document_services();
            }
            if self.document().kind().is_typst() {
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

    fn execute_pending_app_popup_action(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        let Some(action) = self.pending_app_popup_action.take() else {
            return;
        };
        match action {
            AppPopupAction::GitHunk(action, key, chunk) => {
                self.perform_hunk_action(context, action, key, chunk)
            }
            AppPopupAction::Command(command) => {
                let viewport = focused_input_viewport(context);
                if !self.route_edit_command_to_focused_widget(command, context, viewport) {
                    self.execute_app_command(command, context, frame);
                }
            }
            AppPopupAction::Editor(EditorMenuAction::Command(command)) => {
                self.execute_app_command(command, context, frame)
            }
            AppPopupAction::Editor(EditorMenuAction::OpenLink(target)) => {
                self.follow_preview_link(&target)
            }
            AppPopupAction::Editor(EditorMenuAction::EditTable(table)) => {
                self.begin_table_editor(table)
            }
            AppPopupAction::Workspace(action) => match action {
                WorkspaceMenuAction::Open(path) => {
                    if self
                        .document()
                        .path()
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
                WorkspaceMenuAction::Delete(path) => {
                    if !self.document_flow_busy() {
                        self.document_workflow.set_modal(AppModal::DeleteFile {
                            message: format!(
                                "Permanently delete {}? This cannot be undone. Any unsaved edits to this file will also be discarded.",
                                path.display()
                            ),
                            path,
                        });
                        self.set_active_autosave_deadline(None);
                    }
                }
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
        if target.value_range.end > self.document().source().len()
            || !self
                .document()
                .source()
                .is_char_boundary(target.value_range.start)
            || !self
                .document()
                .source()
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
        let selection_end = self.document().source()[..target.value_range.start]
            .chars()
            .count()
            + replacement.chars().count();
        self.document_mut().edit(snapshot.cursor, |source| {
            source.replace_range(target.value_range, &replacement)
        });
        self.pending_editor_selection = Some(EditorSelection::Focus(selection_end..selection_end));
        self.search.clear();
        self.mark_edited();
        self.notice = Some(Notice {
            message: format!("Set document font to {family}"),
            kind: NoticeKind::Success,
        });
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
            PreviewStatus::Ready(_) => ServiceState::Ready("Preview ready".to_owned()),
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
        let preview = self.status_preview();
        if self.document().kind() == DocumentKind::Text && !self.typst_preview_available() {
            return ServiceState::Disabled("Text files do not need a preview renderer".to_owned());
        }
        if self.document().kind() == DocumentKind::Image
            && !self.typst_preview_available()
            && !preview.content.pages().is_empty()
        {
            return ServiceState::Ready("The selected image decoded successfully".to_owned());
        }
        if self.raster_content_freshness() == Some(RasterContentFreshness::Current) {
            return if preview.has_resident_pages() {
                ServiceState::Ready(format!(
                    "Poppler has {} page(s); visible pages render on demand",
                    preview.content.pages().len()
                ))
            } else {
                ServiceState::Starting(format!(
                    "Poppler found {} page(s); rendering the visible range",
                    preview.content.pages().len()
                ))
            };
        }
        if self.raster_content_freshness() == Some(RasterContentFreshness::Stale) {
            return ServiceState::Degraded(format!(
                "Showing {} page(s) from the last successful build",
                preview.content.pages().len()
            ));
        }
        if let Some(error) = preview.content.error() {
            return ServiceState::Degraded(error.to_owned());
        }
        match preview.status {
            PreviewStatus::Error => {
                ServiceState::Failed("No rasterized pages are available".to_owned())
            }
            PreviewStatus::Waiting | PreviewStatus::Compiling | PreviewStatus::Ready(_) => {
                ServiceState::Starting("Waiting for the watched PDF to render".to_owned())
            }
        }
    }

    fn raster_content_freshness(&self) -> Option<RasterContentFreshness> {
        self.status_preview()
            .raster_freshness(self.document().revision())
    }

    fn show_workspace(&mut self, ui: &mut egui::Ui) {
        let _span = crate::performance::span("ui.explorer");
        offer_file_drop_target(
            ui,
            ui.max_rect(),
            FileDropTarget::Folder(self.workspace_root.clone()),
        );
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
            let mut hover = generation.map_or_else(
                || format!("{root}\nDouble-click to change workspace root"),
                |generation| {
                    format!(
                        "{root}\nFilesystem snapshot generation {generation}\nDouble-click to change workspace root"
                    )
                },
            );
            if let Some(warning) = self.project_index.warning() {
                hover.push_str("\nSome Explorer entries could not be discovered.\n");
                hover.push_str(warning);
            }
            if native_hover_text(response, hover).double_clicked() {
                self.open_workspace_chooser();
            }
            if icon_button(ui, UiIcon::Refresh, "Refresh filesystem").clicked() {
                self.refresh_workspace();
            }
        });
        theme::panel_header(ui, "workspace-search-header", |ui| {
            let show_clear = !self.explorer.query().is_empty();
            let clear_width = if show_clear {
                METRICS.icon.button_size.x + ui.spacing().item_spacing.x
            } else {
                0.0
            };
            let search = ui.add_sized(
                [
                    (ui.available_width() - clear_width).max(1.0),
                    METRICS.explorer.header_row_height,
                ],
                egui::TextEdit::singleline(self.explorer.query_mut())
                    .id_salt("explorer-search")
                    .hint_text("Search Explorer"),
            );
            if std::mem::take(&mut self.focus_explorer_search) {
                search.request_focus();
            }
            if show_clear && icon_button(ui, UiIcon::Close, "Clear Explorer search").clicked() {
                self.explorer.query_mut().clear();
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
            self.document().path().as_ref().and_then(|path| {
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
        let mut popup_request = None;
        let context = ui.ctx().clone();
        let explorer_query = normalize_explorer_query(self.explorer.query());
        let filter_active = !explorer_query.is_empty();
        let mut section_defaults = if filter_active {
            explorer_section_query_matches(snapshot, project_index, &explorer_query)
        } else {
            std::array::from_fn(|index| ExplorerSection::ALL[index].default_open())
        };
        let git_in_explorer = self.git.visible;
        section_defaults[ExplorerSection::Git.index()] = git_in_explorer;
        if !git_in_explorer {
            // Keep the hidden section genuinely collapsed. Merely removing
            // it from the height budget still lets a persisted open state
            // render an empty body and consume a frame of layout.
            set_explorer_section_open(ui, "workspace-git", false);
        }
        if git_in_explorer && self.git_explorer_reveal {
            // `CollapsingState` survives across frames, so a Git panel that
            // was previously closed can otherwise remain visually hidden when
            // the user opens Git from the command menu. Reveal it once, then
            // let the user control the section normally.
            set_explorer_section_open(ui, "workspace-git", true);
            self.git_explorer_reveal = false;
        }
        let mut open_sections = explorer_section_open_states(ui, filter_active, section_defaults);
        if !git_in_explorer {
            // A persisted open state must not reserve space for a hidden Git
            // section after the panel has been closed.
            open_sections[ExplorerSection::Git.index()] = false;
        }
        let open_section_count = open_sections.iter().filter(|is_open| **is_open).count();
        let section_frame_height = theme::explorer_section_frame(ui.style())
            .total_margin()
            .sum()
            .y;
        let section_body_budget = available_explorer_section_body_height(
            ui.available_height(),
            open_section_count,
            section_frame_height,
        ) * open_section_count as f32;
        let section_layout_id = explorer_section_layout_id(ui);
        let mut section_layout = ui.ctx().data_mut(|data| {
            data.get_temp::<ExplorerSectionLayout>(section_layout_id)
                .unwrap_or_default()
        });
        let section_body_heights = section_layout.body_heights(open_sections, section_body_budget);
        let order = self.settings.explorer_order;
        let mut open_package_manager = false;
        let mut index_target = None;
        let mut git_output = None;
        let git_dirty = self.document().is_dirty();
        let section_resize = show_explorer_sections(
            ui,
            ExplorerSectionsSpec {
                order,
                defaults: section_defaults,
                heights: section_body_heights,
                open: open_sections,
                filtered: filter_active,
                git_visible: git_in_explorer,
            },
            |ui, section| match section {
                ExplorerSection::Files => {
                    if let Some(snapshot) = &snapshot {
                        // A scan generation describes fresh filesystem data, not a
                        // new UI. Keeping it out of the identity preserves opened
                        // folders, selection, and the surrounding ScrollArea's
                        // offset when a file open triggers a background rescan.
                        let tree_id = workspace_tree_state_id(ui, &snapshot.root, filter_active);
                        let mut tree_state = TreeViewState::load(ui, tree_id).unwrap_or_default();
                        if filter_active {
                            open_matching_workspace_ancestors(
                                &mut tree_state,
                                &snapshot.nodes,
                                &explorer_query,
                            );
                        }
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
                                        &context,
                                        &explorer_query,
                                        &self.git_editor.statuses,
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
                        if filter_active
                            && !snapshot
                                .nodes
                                .iter()
                                .any(|node| workspace_node_matches_query(node, &explorer_query))
                        {
                            ui.label(RichText::new("No matching files").weak());
                        }
                    } else {
                        ui.label(RichText::new("No project folder").weak());
                    }
                    if let Some(error) = &self.workspace_error {
                        ui.colored_label(error_color(ui.ctx()), error);
                    }
                }
                ExplorerSection::Git => {
                    git_output = Some(self.git.show(ui, git_dirty));
                }
                _ => {
                    let outcome = show_project_index_section(
                        ui,
                        section,
                        project_root,
                        project_index,
                        &explorer_query,
                    );
                    open_package_manager |= outcome.open_package_manager;
                    if outcome.target.is_some() {
                        index_target = outcome.target;
                    }
                }
            },
        );

        if let Some(output) = git_output {
            self.git.apply_view_output(ui.ctx(), output);
        }

        if open_package_manager {
            self.open_package_manager(ui.ctx());
        }

        if let Some((section, delta)) = section_resize
            && section_layout.resize_after(
                open_sections,
                section_body_budget,
                order,
                section,
                delta,
            )
        {
            ui.ctx()
                .data_mut(|data| data.insert_temp(section_layout_id, section_layout));
            ui.ctx().request_repaint();
        }

        if let Some(path) = open_path
            && self
                .document()
                .path()
                .as_ref()
                .is_none_or(|current| !same_path(current, &path))
        {
            self.request_document_replacement(
                DeferredDocumentAction::LoadPath(path),
                "opening another project file",
            );
        }
        if let Some((path, line)) = index_target {
            self.navigate_file_location(
                path,
                None,
                Some((line, 1)),
                "opening a project index entry",
            );
        }
        if let Some(popup) = popup_request {
            self.open_app_popup(popup);
        }
    }

    fn dismiss_hover_on_scroll(&mut self, context: &egui::Context) {
        self.tooltip_request = None;
        self.editor_hover = None;
        self.diagnostic_tooltip = None;
        self.clear_asset_hover();
        clear_native_hover_overlay(context);
        clear_asset_hover_candidate(context);
        let geometry_id = tooltip_geometry_id(context);
        let interaction_id = tooltip_interaction_id(context);
        context.data_mut(|data| {
            data.remove::<TooltipGeometry>(geometry_id);
            data.remove::<TooltipInteractionState>(interaction_id);
        });
    }

    fn dismiss_keyboard_tooltip(&mut self, context: &egui::Context) {
        self.tooltip_request = None;
        self.editor_hover = None;
        self.diagnostic_tooltip = None;
        self.clear_asset_hover();
        clear_native_hover_overlay(context);
        let id = tooltip_interaction_id(context);
        let geometry_id = tooltip_geometry_id(context);
        let dismissed_id = viewport_scoped_id(context, "dismissed-tooltip-origin");
        context.data_mut(|data| {
            if let Some(geometry) = data.get_temp::<TooltipGeometry>(geometry_id) {
                data.insert_temp(dismissed_id, geometry.origin);
            }
            if let Some(mut state) = data.get_temp::<TooltipInteractionState>(id) {
                state.dismissed = true;
                state.focused = false;
                state.focus_requested = false;
                data.insert_temp(id, state);
            }
        });
        context.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn focus_requested_tooltip(&mut self, context: &egui::Context) {
        if self.tooltip_request.is_none() {
            return;
        }
        let overlay_id = native_hover_tooltip_id(context);
        let identity = self
            .asset_hover
            .as_ref()
            .map(|hover| asset_tooltip_identity(hover.origin, &hover.path))
            .or_else(|| {
                self.diagnostic_tooltip
                    .as_ref()
                    .map(|tooltip| tooltip_identity(tooltip.origin, &tooltip.detail))
            })
            .or_else(|| {
                context.data(|data| {
                    data.get_temp::<HoverTooltipOverlay>(overlay_id)
                        .map(|tooltip| tooltip_identity(tooltip.origin, &tooltip.detail))
                })
            });
        if let Some(identity) = identity {
            let mut state = TooltipInteractionState::new(identity);
            state.focus_requested = true;
            let id = tooltip_interaction_id(context);
            context.data_mut(|data| data.insert_temp(id, state));
            self.tooltip_request = None;
        }
    }

    fn update_editor_hover(&mut self, ui: &mut egui::Ui, hovered: Option<(Range<usize>, Rect)>) {
        let timing_id = native_hover_tooltip_id(ui.ctx()).with("semantic-hover-timing");
        let Some((range, rect)) = hovered else {
            self.editor_hover = None;
            reset_hover_timing(ui.ctx(), timing_id);
            return;
        };
        // A different hover target may lie under the pointer while it travels
        // toward an already visible native tooltip. Do not let that target
        // replace the active payload during the handoff.
        if native_tooltip_handoff_blocks(ui.ctx(), rect.expand(theme::SPACE.tight)) {
            return;
        }
        let (Some(generation), Some(uri)) = (
            self.tinymist_sync.generation,
            self.tinymist_sync.current_uri.clone(),
        ) else {
            self.editor_hover = None;
            return;
        };
        let version = revision_as_i32(self.document().revision());
        let changed = self.editor_hover.as_ref().is_none_or(|hover| {
            hover.range != range || hover.uri != uri || hover.version != version
        });
        if changed {
            let request_token = self.next_editor_hover_token;
            self.next_editor_hover_token = self.next_editor_hover_token.wrapping_add(1).max(1);
            self.editor_hover = Some(EditorHoverState {
                key: self.document().key(),
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
        let opacity = if self.tooltip_request.is_some() {
            Some(1.0)
        } else {
            hover_opacity(&response, timing_id)
        };

        let should_request = hover_request_ready(
            opacity.is_some(),
            self.preview.connection.is_ready(),
            self.tinymist_sync.current_open,
            self.editor_hover
                .as_ref()
                .is_none_or(|hover| hover.requested),
        );
        if should_request {
            let position = if self.document().config().is_none() {
                Some(lsp_position_at_scalar(
                    self.document().source(),
                    ScalarOffset::new(range.start),
                ))
            } else {
                self.document()
                    .canonical_snapshot()
                    .ok()
                    .and_then(|snapshot| {
                        snapshot.canonical_lsp_position(ScalarOffset::new(range.start))
                    })
            };
            let Some(position) = position else {
                return;
            };
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

    fn request_editor_completion(&mut self, cursor: usize, anchor: Rect, explicit: bool) {
        self.prepare_editor_source_data();
        if self.document().kind().is_typst()
            && let Some(all_items) = self.editor_data.tex_completions(cursor).or_else(|| {
                crate::completion::font_items(self.document().source(), cursor, &self.font_catalog)
            })
        {
            let items = crate::completion::filtered_for_source(
                &all_items,
                self.document().source(),
                cursor,
            );
            self.editor_completion = Some(EditorCompletionState {
                key: self.document().key(),
                generation: self.tinymist_sync.generation.unwrap_or(Generation(0)),
                uri: self.tinymist_sync.current_uri.clone().unwrap_or_default(),
                version: revision_as_i32(self.document().revision()),
                request_token: 0,
                cursor,
                source_cursor: cursor,
                anchor,
                explicit,
                is_incomplete: false,
                selected: 0,
                items,
                all_items,
                source: self.document().source().clone(),
                local: true,
            });
            return;
        }
        if !explicit
            && self
                .document()
                .source()
                .chars()
                .nth(cursor.saturating_sub(1))
                .is_some_and(|c| c.is_whitespace() || c == '"')
        {
            self.editor_completion = None;
            return;
        }
        let ready = tinymist_language_features_ready(
            self.document().kind(),
            self.preview.connection.is_ready(),
            self.tinymist_sync.current_open,
        );
        let (Some(generation), Some(uri)) = (
            self.tinymist_sync.generation,
            self.tinymist_sync.current_uri.clone(),
        ) else {
            self.editor_completion = None;
            if explicit {
                self.notice = Some(Notice {
                    message: "Completions are unavailable until Tinymist is ready".to_owned(),
                    kind: NoticeKind::Info,
                });
            }
            return;
        };
        if !ready {
            self.editor_completion = None;
            if explicit {
                self.notice = Some(Notice {
                    message: "Completions are unavailable until Tinymist is ready".to_owned(),
                    kind: NoticeKind::Info,
                });
            }
            return;
        }

        let cursor = cursor.min(self.document().source().chars().count());
        let version = revision_as_i32(self.document().revision());
        let request_token = self.next_editor_completion_token;
        self.next_editor_completion_token =
            self.next_editor_completion_token.wrapping_add(1).max(1);
        let (position, source_cursor, source) = if self.document().config().is_none() {
            (
                lsp_position_at_scalar(self.document().source(), ScalarOffset::new(cursor)),
                cursor,
                self.document().source().clone(),
            )
        } else {
            let snapshot = match self.document().canonical_snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    self.editor_completion = None;
                    if explicit {
                        self.notice = Some(Notice {
                            message: error.to_string(),
                            kind: NoticeKind::Info,
                        });
                    }
                    return;
                }
            };
            let Some(position) = snapshot.canonical_lsp_position(ScalarOffset::new(cursor)) else {
                return;
            };
            let Some(source_cursor) = snapshot.canonical_scalar_cursor(ScalarOffset::new(cursor))
            else {
                return;
            };
            (position, source_cursor.get(), snapshot.source().to_owned())
        };
        match self.tinymist.complete_document(
            generation,
            uri.clone(),
            version,
            position,
            request_token,
        ) {
            Ok(()) => {
                self.editor_completion = Some(EditorCompletionState {
                    key: self.document().key(),
                    generation,
                    uri,
                    version,
                    request_token,
                    cursor,
                    source_cursor,
                    anchor,
                    explicit,
                    is_incomplete: false,
                    selected: 0,
                    items: self
                        .editor_completion
                        .as_ref()
                        .map(|state| state.items.clone())
                        .unwrap_or_default(),
                    all_items: self
                        .editor_completion
                        .as_ref()
                        .map(|state| state.all_items.clone())
                        .unwrap_or_default(),
                    source,
                    local: false,
                });
            }
            Err(error) => {
                self.editor_completion = None;
                if explicit {
                    self.notice = Some(Notice {
                        message: format!("Could not request completions: {error}"),
                        kind: NoticeKind::Error,
                    });
                }
            }
        }
    }

    fn receive_editor_completions(
        &mut self,
        response: EditorCompletionResponse,
        context: &egui::Context,
    ) {
        let EditorCompletionResponse {
            generation,
            uri,
            version,
            request_token,
            is_incomplete,
            mut items,
        } = response;
        let document_revision = self.document().revision();
        let document_key = self.document().key();
        let request_key = self
            .editor_completion
            .as_ref()
            .map_or(document_key, |completion| completion.key);
        let sync_current = self.tinymist_sync.accepts_reply(
            crate::tinymist_sync::ReplyIdentity {
                generation,
                uri: &uri,
                version,
                key: request_key,
            },
            document_key,
        );
        let Some(completion) = &mut self.editor_completion else {
            return;
        };
        let current = sync_current
            && completion_response_matches(
                completion,
                generation,
                &uri,
                version,
                request_token,
                self.tinymist_sync.generation,
                self.tinymist_sync.current_uri.as_deref(),
                revision_as_i32(document_revision),
            );
        if !current || completion.key != document_key {
            return;
        }
        items.retain(|item| !item.label.trim().is_empty());
        items.sort_by(|left, right| {
            left.sort_text
                .as_deref()
                .unwrap_or(&left.label)
                .cmp(right.sort_text.as_deref().unwrap_or(&right.label))
                .then_with(|| left.label.cmp(&right.label))
        });
        let all_items = items;
        let items = crate::completion::filtered_for_source(
            &all_items,
            &completion.source,
            completion.source_cursor,
        );
        if items.is_empty() {
            self.editor_completion = None;
            return;
        }
        completion.is_incomplete = is_incomplete;
        completion.selected = 0;
        completion.items = items;
        completion.all_items = all_items;
        context.request_repaint();
    }

    fn apply_editor_completion(&mut self, index: usize, context: &egui::Context) {
        let Some(completion) = self.editor_completion.as_ref() else {
            return;
        };
        let current = (completion.local
            || (self.tinymist_sync.generation == Some(completion.generation)
                && self.tinymist_sync.current_uri.as_deref() == Some(completion.uri.as_str())))
            && completion.version == revision_as_i32(self.document().revision());
        if !current || completion.key != self.document().key() {
            self.editor_completion = None;
            return;
        }
        let Some(item) = completion.items.get(index).cloned() else {
            return;
        };
        let coordinates = if completion.local {
            crate::completion_edit::CompletionCoordinates::Display
        } else {
            crate::completion_edit::CompletionCoordinates::Canonical
        };
        let transaction = crate::completion_edit::CompletionTransaction::prepare(
            completion.key,
            &completion.source,
            completion.source_cursor,
            &item,
            coordinates,
        );
        let snapshot = self.editor_snapshot(context);
        let selection = transaction
            .and_then(|transaction| transaction.commit(self.document_mut(), snapshot.cursor));
        let selection = match selection {
            Ok(selection) => selection,
            Err(error) => {
                self.editor_completion = None;
                self.notice = Some(Notice {
                    message: format!("Could not apply completion: {error}"),
                    kind: NoticeKind::Error,
                });
                return;
            }
        };
        self.pending_editor_selection = Some(EditorSelection::Focus(selection));
        self.search.clear();
        self.editor_completion = None;
        self.mark_edited();
        self.notice = Some(Notice {
            message: format!("Completed {}", item.label),
            kind: NoticeKind::Success,
        });
        let editor_id = source_editor_id(context);
        context.memory_mut(|memory| memory.request_focus(editor_id));
        context.request_repaint();
    }

    fn handle_editor_completion_keys(&mut self, context: &egui::Context) {
        // Enter at a block opener creates the body, even when automatic
        // language-name suggestions are visible. Tab still accepts them.
        if self.editor_completion.is_some()
            && self.settings.auto_pair_delimiters
            && self.document().kind().is_typst()
            && context.input(|input| {
                input.key_pressed(egui::Key::Enter) && input.modifiers == Modifiers::NONE
            })
            && let Some(range) =
                egui::text_edit::TextEditState::load(context, source_editor_id(context))
                    .and_then(|state| state.cursor.char_range())
            && range.primary.index == range.secondary.index
        {
            let source = self.tabs.current_record().document.source();
            let byte = source
                .char_indices()
                .nth(range.primary.index.0)
                .map_or(source.len(), |(byte, _)| byte);
            if self.auto_pair_syntax.newline(source, byte).is_some() {
                self.editor_completion = None;
                return;
            }
        }
        let Some(completion) = &mut self.editor_completion else {
            return;
        };
        if context.input_mut(|input| input.consume_key(Modifiers::NONE, egui::Key::Escape)) {
            self.editor_completion = None;
            return;
        }
        if completion.items.is_empty() {
            return;
        }
        let len = completion.items.len();
        if context.input_mut(|input| input.consume_key(Modifiers::NONE, egui::Key::ArrowDown)) {
            completion.selected = (completion.selected + 1) % len;
            context.request_repaint();
        }
        if context.input_mut(|input| input.consume_key(Modifiers::NONE, egui::Key::ArrowUp)) {
            completion.selected = completion.selected.checked_sub(1).unwrap_or(len - 1);
            context.request_repaint();
        }
        let accept = context.input_mut(|input| {
            input.consume_key(Modifiers::NONE, egui::Key::Enter)
                || input.consume_key(Modifiers::NONE, egui::Key::Tab)
        });
        if accept {
            let selected = completion.selected;
            self.apply_editor_completion(selected, context);
        }
    }

    fn show_preview(&mut self, ui: &mut egui::Ui, frame: Option<&eframe::Frame>) {
        if self.document().kind().preview_only() && !self.typst_preview_available() {
            self.hide_webview();
            self.show_asset_view(ui);
            return;
        }
        let status = self.preview_status_snapshot();
        let native_ready = status.native_ready;
        let native_transitioning = status.interactive_transitioning;
        let should_attempt_native = status.should_attempt_native;
        let canonical_artifact_available = status.canonical_artifact_available;
        // The interactive viewer needs no toolbar, so its content aligns
        // exactly with the code panel. Raster-only page controls get one fixed
        // row and never change height with build status.
        if !native_ready && !native_transitioning {
            theme::panel_header(ui, "preview-header", |ui| {
                self.show_preview_header_controls(ui, native_ready);
            });
        }

        // WKWebView is a separate native layer and cannot be read by egui's
        // app-window framebuffer capture. Use the matching rasterised page for
        // this one QA frame; no desktop capture API is involved.
        let main_capture_pending = self.captures.has_pending_for("main");
        if main_capture_pending {
            if capture_preview_build_needed(
                main_capture_pending,
                self.preview.has_resident_pages(),
                self.compile_deadline.is_some(),
                self.preview.status,
                canonical_artifact_available,
            ) {
                self.schedule_compile_now();
            }
            self.hide_webview();
            self.show_native_preview(ui);
            return;
        }

        if should_attempt_native {
            // The interactive viewer is a native child view, so it does not
            // inherit egui's clip rectangle. Keep its bounds inside the
            // preview pane or it can draw over the editor after a resize.
            let available = ui.available_rect_before_wrap();
            let clip = ui.clip_rect();
            let rect = clipped_preview_rect(available, clip);
            let Some(native_rect) = egui_rect_to_native(ui.ctx(), rect) else {
                self.hide_webview();
                return;
            };
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
            } else if native_transitioning {
                self.hide_webview();
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                let waiting_for_focus = self.webview.is_none()
                    && !may_create_window_webview(
                        self.window_host,
                        cfg!(target_os = "macos"),
                        ui.ctx().input(|input| input.viewport().focused),
                    );
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                let waiting_for_focus = false;
                show_preview_transition(ui, waiting_for_focus);
            } else {
                self.hide_webview();
                self.show_native_preview(ui);
            }
        } else if native_transitioning {
            self.hide_webview();
            show_preview_transition(ui, false);
        } else {
            self.hide_webview();
            self.show_native_preview(ui);
        }
    }

    fn show_preview_header_controls(&mut self, ui: &mut egui::Ui, native_ready: bool) {
        if !native_ready {
            let fresh = self.raster_content_freshness() == Some(RasterContentFreshness::Current);
            raster_view::show_controls(
                ui,
                &mut self.preview,
                fresh,
                snapshot_scene_hides_preview_pages(self.snapshot_scene),
            );
        }
    }

    fn show_native_preview(&mut self, ui: &mut egui::Ui) {
        let fresh = self.raster_content_freshness() == Some(RasterContentFreshness::Current);
        if let Some(target) = raster_view::show_pages(
            ui,
            &mut self.preview,
            fresh,
            snapshot_scene_hides_preview_pages(self.snapshot_scene),
        ) {
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
                    let color = diagnostic_color(diagnostic.severity, ui.ctx());
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
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!(
                                                "{}:{}",
                                                location.line, location.column
                                            ))
                                            .monospace()
                                            .color(ui.visuals().weak_text_color()),
                                        )
                                        .selectable(true),
                                    );
                                }
                                ui.add(
                                    egui::Label::new(&diagnostic.message)
                                        .selectable(true)
                                        .wrap(),
                                );
                            });
                            for detail in &diagnostic.details {
                                ui.horizontal(|ui| {
                                    ui.add_space(METRICS.problems.detail_indent);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(detail)
                                                .size(theme::TYPE.supporting)
                                                .monospace()
                                                .weak(),
                                        )
                                        .selectable(true)
                                        .wrap(),
                                    );
                                });
                            }
                        })
                        .response;
                    let row = if self.snapshot_scene == Some(UiSnapshotScene::ProblemsPanel)
                        && index == 0
                    {
                        row.highlight()
                    } else {
                        row
                    };
                    let pointer_in_row = row
                        .ctx
                        .pointer_latest_pos()
                        .is_some_and(|position| row.rect.contains(position));
                    if pointer_in_row || row.highlighted() {
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
                        let hover = ui.interact(
                            row.rect,
                            ui.id().with(("problem-row-hover", index)),
                            Sense::hover(),
                        );
                        native_hover_text(hover, "Double-click to open this location");
                        if let Some(diagnostic) =
                            problem_row_jump_target(diagnostic, problem_row_double_clicked(&row))
                        {
                            jump_target = Some(diagnostic);
                        }
                    }
                }
                if !self.preview.raw_diagnostics.is_empty() {
                    ui.collapsing("Raw CLI stdout", |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.preview.raw_diagnostics).monospace(),
                            )
                            .selectable(true)
                            .wrap(),
                        );
                    });
                }
            });
        if let Some(diagnostic) = jump_target {
            self.jump_to_diagnostic(diagnostic);
        }
    }

    fn record_status_transition(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let status = self.status_preview().status;
        if self.recorded_status == Some(status) {
            return;
        }
        let kind = match status {
            PreviewStatus::Ready(_) => NoticeKind::Success,
            PreviewStatus::Error => NoticeKind::Error,
            PreviewStatus::Waiting | PreviewStatus::Compiling => NoticeKind::Info,
        };
        self.push_status_log(self.status_detail(), kind);
        self.recorded_status = Some(status);
    }

    fn record_notice_transition(&mut self) {
        let Some(notice) = self.notice.as_ref() else {
            self.recorded_notice = None;
            return;
        };
        if self.recorded_notice.as_ref() == Some(notice) {
            return;
        }
        let notice = notice.clone();
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
        if self.document().kind().is_typst() {
            return None;
        }
        self.document()
            .path()
            .as_ref()
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
            .or_else(|| {
                Some(
                    match self.document().kind() {
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
        if !self.document().kind().is_editable() {
            return None;
        }
        let char_index = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .and_then(|state| state.cursor.char_range())
            .map_or(0, |range| range.primary.index.0)
            .min(self.document().source().chars().count());
        Some(line_column_at_char(self.document().source(), char_index))
    }

    fn git_line_change_counts(&self) -> Option<crate::git::repository::diff::LineChangeCounts> {
        let path = self
            .document()
            .path()
            .as_deref()
            .filter(|_| self.document().kind().is_editable());
        self.git_editor
            .has_gutter(path)
            .then(|| self.git_editor.line_change_counts())
    }

    fn git_line_change_summary(
        context: &egui::Context,
        counts: crate::git::repository::diff::LineChangeCounts,
    ) -> egui::text::LayoutJob {
        use crate::git::repository::diff::ChangeKind;
        let mut job = egui::text::LayoutJob::default();
        let format = egui::TextFormat {
            font_id: egui::FontId::proportional(theme::TYPE.supporting),
            color: theme::palette(context).neutral,
            ..Default::default()
        };
        job.append("Git: ", 0.0, format.clone());
        for (index, (symbol, count, kind)) in [
            ("+", counts.added, ChangeKind::Added),
            ("~", counts.modified, ChangeKind::Modified),
            ("-", counts.deleted, ChangeKind::Deleted),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                job.append(" · ", 0.0, format.clone());
            }
            job.append(
                &format!("{symbol}{count}"),
                0.0,
                egui::TextFormat {
                    color: kind.color(context),
                    ..format.clone()
                },
            );
        }
        job
    }

    fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        if self.tabs.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.weak("Workspace ready · no open tabs");
            });
            return;
        }
        self.record_status_transition();
        self.record_notice_transition();
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
            if self.document().kind().is_editable() {
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

            let (icon, color, _) = if !self.may_run_compilation() && self.typst_preview_available()
            {
                (UiIcon::Waiting, neutral_color(ui.ctx()), None)
            } else {
                match self.status_preview().status {
                    PreviewStatus::Waiting => (UiIcon::Waiting, neutral_color(ui.ctx()), None),
                    PreviewStatus::Compiling => (UiIcon::Refresh, info_color(ui.ctx()), None),
                    PreviewStatus::Ready(elapsed) => (
                        UiIcon::Check,
                        success_color(ui.ctx()),
                        preview_timing_label(self.document().kind(), elapsed),
                    ),
                    PreviewStatus::Error => (UiIcon::Warning, error_color(ui.ctx()), None),
                }
            };
            let status_detail = self.status_detail();
            let (message, kind) = self
                .status_log
                .front()
                .map_or((status_detail.as_str(), NoticeKind::Info), |entry| {
                    (entry.detail.as_str(), entry.kind)
                });
            let message_color = match kind {
                NoticeKind::Info => info_color(ui.ctx()),
                NoticeKind::Success => success_color(ui.ctx()),
                NoticeKind::Error => error_color(ui.ctx()),
            };
            let status_response = ui
                .horizontal(|ui| {
                    if matches!(self.status_preview().status, PreviewStatus::Ready(_))
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
                    ui.add(
                        egui::Label::new(
                            RichText::new(message)
                                .size(theme::TYPE.supporting)
                                .color(message_color),
                        )
                        .truncate(),
                    );
                })
                .response
                .interact(Sense::click());
            native_hover_text(
                status_response.clone(),
                format!("{message}\nDouble-click for recent status"),
            );
            if status_response.double_clicked() {
                self.open_app_popup(AppPopup::StatusLog {
                    anchor: status_response.rect.left_top(),
                });
            }
            if !fallbacks.is_empty() {
                ui.separator();
                let count = fallbacks.len();
                let color = warning_color(ui.ctx());
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
            if let Some(counts) = self.git_line_change_counts() {
                ui.separator();
                let summary = Self::git_line_change_summary(ui.ctx(), counts);
                native_hover_text(
                    ui.label(summary),
                    format!(
                        "Git changes in this document\n{} added, {} modified, {} deleted",
                        counts.added, counts.modified, counts.deleted
                    ),
                );
            }
        });
    }

    fn status_detail(&self) -> String {
        if self.tabs.is_empty() {
            return "Workspace ready · no open tabs".into();
        }
        if !self.may_run_compilation() && self.typst_preview_available() {
            return "Automatic preview updates paused".to_owned();
        }
        if !self.typst_preview_available() {
            return match (self.document().kind(), self.status_preview().status) {
                (DocumentKind::Text, _) => "Text file ready".to_owned(),
                (DocumentKind::Image, PreviewStatus::Compiling) => "Decoding image".to_owned(),
                (DocumentKind::Image, PreviewStatus::Ready(_)) => "Image ready".to_owned(),
                (DocumentKind::Pdf, PreviewStatus::Compiling) => "Rendering PDF".to_owned(),
                (DocumentKind::Pdf, PreviewStatus::Ready(_)) => "Preview ready".to_owned(),
                (_, PreviewStatus::Error) => self
                    .notice
                    .as_ref()
                    .map(|notice| notice.message.clone())
                    .unwrap_or_else(|| "Could not open file".to_owned()),
                _ => "Opening file".to_owned(),
            };
        }
        match self.preview.status {
            PreviewStatus::Waiting => "Preview build queued".to_owned(),
            PreviewStatus::Compiling => "Compiling preview".to_owned(),
            PreviewStatus::Ready(elapsed) => preview_timing_label(DocumentKind::Typst, elapsed)
                .map_or_else(
                    || "Preview ready".to_owned(),
                    |timing| format!("Preview ready in {timing}"),
                ),
            PreviewStatus::Error => self
                .preview
                .raw_diagnostics
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("Preview build failed")
                .to_owned(),
        }
    }

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
            self.webview_applied = None;
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

    pub(crate) fn has_settings_update(&self) -> bool {
        self.pending_settings.is_some()
    }

    pub(crate) fn has_shell_work(&self) -> bool {
        self.has_settings_update()
            || self.settings_open_requested
            || !self.pending_window_requests.is_empty()
            || !self.workspace_history_removals.is_empty()
            || self.process_close_answer().is_some()
    }

    pub(crate) fn shell_signal(&self) -> (u64, bool, bool) {
        (
            self.document().key().epoch,
            self.is_dirty(),
            self.interactive_preview_active(),
        )
    }

    pub(crate) fn merge_settings_update(&mut self, target: &mut AppSettings) {
        if let Some(edited) = self.pending_settings.take() {
            target.apply_edits(&self.settings, edited);
        }
    }

    pub(crate) fn take_workspace_history_removals(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.workspace_history_removals)
    }

    pub(crate) fn apply_workspace_history_removals(&mut self, roots: &[PathBuf]) {
        for root in roots {
            self.settings.forget_workspace(root);
            if let Some(settings) = &mut self.pending_settings {
                settings.forget_workspace(root);
            }
        }
    }

    pub(crate) fn apply_shared_settings(&mut self, settings: AppSettings, context: &egui::Context) {
        let enable_tex = settings.mitex_auto_enable && !self.settings.mitex_auto_enable;
        let autosave_changed = settings.auto_save != self.settings.auto_save
            || settings.auto_save_delay_ms != self.settings.auto_save_delay_ms;
        self.settings = settings;
        if enable_tex && self.tex_mode_available() {
            self.set_tex_mode(true, context);
        }
        if autosave_changed {
            self.reschedule_parked_autosave();
            let deadline = (self.settings.auto_save
                && self.document().path().is_some()
                && self.is_dirty())
            .then(|| {
                Instant::now() + Duration::from_millis(self.settings.auto_save_delay_ms.max(100))
            });
            self.set_active_autosave_deadline(deadline);
        }
        context.request_repaint();
    }
}

fn problem_row_jump_target(diagnostic: &Diagnostic, double_clicked: bool) -> Option<Diagnostic> {
    (double_clicked && diagnostic.location.is_some()).then(|| diagnostic.clone())
}

fn problem_row_double_clicked(response: &egui::Response) -> bool {
    response
        .ctx
        .pointer_latest_pos()
        .is_some_and(|position| response.rect.contains(position))
        && response.ctx.input(|input| {
            input
                .pointer
                .button_double_clicked(egui::PointerButton::Primary)
        })
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
        self.ui_in_window(ui, Some(frame));
    }
}

impl EditorApp {
    /// Root passes supply their native frame. Deferred documents use their
    /// own retained platform parent, never the root's frame/window handle.
    pub(crate) fn ui_in_window(&mut self, ui: &mut egui::Ui, frame: Option<&eframe::Frame>) {
        let _span = crate::performance::span("ui.editor.pass");
        let context = ui.ctx().clone();
        ChildViewHost::resume_owner(&context);
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
        );
        self.captures.set_manual_shortcut_override(
            self.settings
                .shortcut_overrides
                .get(ShortcutAction::CaptureUi)
                .map(|binding| binding.map(ShortcutChord::egui)),
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
            if self.table_editor_suspended {
                self.table_editor_had_focus = false;
                self.table_editor_suspended = false;
            }
        }
        // The GL surface is alpha-capable for child popup viewports. Keep the
        // main window itself fully opaque by painting its complete root first.
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let native_tooltip_id = native_hover_tooltip_id(&context);
        let geometry_id = tooltip_geometry_id(&context);
        let interaction_id = tooltip_interaction_id(&context);
        let dismissed_id = viewport_scoped_id(&context, "dismissed-tooltip-origin");
        let pointer = context.pointer_latest_pos();
        context.data_mut(|data| {
            if data
                .get_temp::<TooltipInteractionState>(interaction_id)
                .is_some_and(|state| state.dismissed)
                && let Some(geometry) = data.get_temp::<TooltipGeometry>(geometry_id)
            {
                data.insert_temp(dismissed_id, geometry.origin);
            }
            if data
                .get_temp::<Rect>(dismissed_id)
                .is_some_and(|origin| pointer.is_some_and(|pointer| !origin.contains(pointer)))
            {
                data.remove::<Rect>(dismissed_id);
            }
        });
        let scroll_dismissed = update_hover_scroll(&context);
        if scroll_dismissed {
            self.dismiss_hover_on_scroll(&context);
        }
        let tooltip_retained = !scroll_dismissed && native_tooltip_handoff_active(&context, true);
        clear_asset_hover_candidate(&context);
        if !tooltip_retained {
            self.diagnostic_tooltip = None;
            self.clear_asset_hover();
            context.data_mut(|data| {
                data.remove::<HoverTooltipOverlay>(native_tooltip_id);
                data.remove::<TooltipGeometry>(geometry_id);
                data.remove::<TooltipInteractionState>(interaction_id);
            });
        }
        self.process_open_requests(&context);
        self.execute_pending_document_action(&context, frame);
        self.poll_font_catalog(&context);
        self.poll_package_catalog(&context);
        self.sync_runtime_settings(&context);
        self.receive_compile_results();
        self.receive_asset_results(&context);
        self.receive_pdf_page_results(&context);
        self.receive_asset_thumbnail_results(&context);
        self.poll_export_dialog(&context);
        self.poll_tool_picker(&context);
        self.poll_document_dialog(&context);
        self.receive_tinymist_events(&context);
        self.tick_tinymist_recovery(&context);
        self.receive_web_links();
        self.handle_shortcuts(&context, frame);
        // Menu-driven TextEdit commands may inject semantic events. Process
        // them after shortcut normalization so they reach the focused widget
        // unchanged during the UI pass below.
        self.process_native_menu_commands(&context, frame);
        if self.lifecycle.needs_visible_window() {
            // A New/Open queued during dormancy replaces the buffer before
            // services start, avoiding a throwaway untitled service generation.
            self.execute_pending_document_action(&context, frame);
            if self.lifecycle.activate() {
                self.reset_document_services();
                self.schedule_compile_now();
            }
        }
        self.execute_pending_app_popup_action(&context, frame);
        self.poll_hunk_action();
        self.poll_save(&context);
        self.tick_parked_autosave(&context);
        let drop_id = viewport_scoped_id(&context, "file-drop-target");
        context.data_mut(|data| data.remove::<FileDropTarget>(drop_id));
        let force_hover_id = viewport_scoped_id(&context, "force-pointer-tooltip");
        let force_hover = matches!(self.tooltip_request, Some(TooltipRequest::Pointer(pos)) if context.pointer_latest_pos() == Some(pos));
        context.data_mut(|data| data.insert_temp(force_hover_id, force_hover));
        self.handle_close_request(&context);
        self.update_title(&context);
        self.tick_workspace(&context);
        self.tick_project_index(&context);
        self.prepare_qa_scene(&context);
        if self.snapshot_scene.is_none() {
            // Advance the Git model independently of the Explorer body's
            // collapsed state. A collapsed section still needs to collect
            // completed jobs and propagate fresh file/gutter status.
            self.git.poll(&context, &self.workspace_root);
            if self.git.take_status_changed() {
                self.git_editor.request_refresh();
            }
            let document = &self.tabs.current_record().document;
            let projected = document.config().is_some();
            let projection = if projected {
                document.canonical_snapshot().ok()
            } else {
                None
            };
            self.git_editor.tick(
                &context,
                &self.workspace_root,
                document.path().as_deref().filter(|_| {
                    document.kind().is_editable() && (!projected || projection.is_some())
                }),
                document.key(),
                document.source(),
                projection.as_ref(),
            );
            if matches!(self.app_popup, Some(AppPopup::GitChunk { .. }))
                && self.git_editor.chunk.is_none()
            {
                self.close_app_popup();
            }
        }
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
        if self.explorer.panel_visible() {
            let panel_id = explorer_panel_id(&context);
            let persisted_width = egui::PanelState::load(&context, panel_id)
                .map_or(METRICS.chrome.explorer_default_width, |state| {
                    state.size().x
                });
            let startup_width = self
                .explorer
                .startup_width(persisted_width, METRICS.chrome.explorer_default_width);
            if let Some(width) = startup_width.or_else(|| self.explorer.take_restored_width())
                && let Some(state) = egui::PanelState::load(&context, panel_id)
            {
                let outer_rect = explorer_width_restored_rect(state.outer_rect, width);
                context.data_mut(|data| {
                    data.insert_persisted(panel_id, egui::PanelState { outer_rect });
                });
            }
            let show_contents = self.explorer.contents_visible();
            egui::Panel::left(panel_id)
                .frame(theme::content_panel_frame(ui.style()))
                .resizable(true)
                .default_size(METRICS.chrome.explorer_default_width)
                .min_size(METRICS.chrome.explorer_min_width)
                .show(ui, |ui| {
                    if show_contents {
                        self.show_workspace(ui);
                    }
                });
            if show_contents && let Some(state) = egui::PanelState::load(&context, panel_id) {
                self.explorer.remember_width(state.size().x);
            }
            if self.explorer.finish_frame() {
                context.request_repaint();
            }
        }
        use workspace_view::ContentView;
        let content_view = workspace_view::content_view(
            self.tabs.is_empty(),
            self.document().kind(),
            self.typst_preview_available(),
            self.view_mode,
        );
        match content_view {
            ContentView::Empty | ContentView::Source | ContentView::Asset => {
                self.hide_webview();
                egui::CentralPanel::default()
                    .frame(theme::content_panel_frame(ui.style()))
                    .show(ui, |ui| match content_view {
                        ContentView::Empty => self.show_empty_workspace(ui, frame),
                        ContentView::Asset => self.show_asset_view(ui),
                        _ => self.show_editor(ui),
                    });
            }
            ContentView::SplitSource | ContentView::SplitAsset => {
                let layout = theme::split_pane_layout(ui.available_width());
                egui::Panel::left("editor")
                    .frame(theme::content_panel_frame(ui.style()))
                    .resizable(true)
                    .default_size(layout.editor_width)
                    .min_size(layout.editor_minimum)
                    .max_size(layout.editor_maximum)
                    .show(ui, |ui| {
                        if content_view == ContentView::SplitAsset {
                            self.show_asset_view(ui);
                        } else {
                            self.show_editor(ui);
                        }
                    });
                egui::CentralPanel::default()
                    .frame(theme::content_panel_frame(ui.style()))
                    .show(ui, |ui| self.show_preview(ui, frame));
            }
            ContentView::Preview => {
                egui::CentralPanel::default()
                    .frame(theme::content_panel_frame(ui.style()))
                    .show(ui, |ui| self.show_preview(ui, frame));
            }
        }
        self.handle_dropped_file(&context);
        self.update_asset_hover(&context);
        self.focus_requested_tooltip(&context);
        self.show_app_popup_window(&context);
        self.show_rename_dialog(&context);
        self.show_table_editor_window(&context);
        self.show_app_modal_window(&context);
        self.show_asset_hover_window(&context);
        self.show_diagnostic_tooltip_window(&context);
        self.show_settings_window(&context, frame);
        self.show_shortcut_editor_window(&context);
        self.show_typst_overrides_window(&context);
        self.show_workspace_chooser(&context);
        self.show_package_manager_window(&context);
        self.reconcile_child_view_lifecycles(&context);
        // Every transaction is observed, including edits introduced by new
        // commands which do not explicitly request immediate service updates.
        self.mark_edited();
        self.sync_preview_visibility();
        self.tick_autosave(&context);
        self.tick_compile(&context);
        self.tick_pdf_page_requests();
    }

    fn reconcile_child_view_lifecycles(&self, context: &egui::Context) {
        let overlay_id = native_hover_tooltip_id(context);
        let retained_hover =
            context.data(|data| data.get_temp::<HoverTooltipOverlay>(overlay_id).is_some());
        for (visible, salt) in [
            (self.shortcut_editor_visible, "tiptoptyp-shortcuts"),
            (self.typst_overrides_visible, "tiptoptyp-typst-overrides"),
            (self.packages_visible, "tiptoptyp-packages"),
            (self.app_popup.is_some(), "tiptoptyp-popup-overlay"),
            (self.asset_hover.is_some(), "asset-hover-overlay"),
            (
                self.diagnostic_tooltip.is_some() || retained_hover,
                "diagnostic-tooltip-overlay",
            ),
        ] {
            if !visible {
                ChildViewHost::close(context, salt);
            }
        }
    }
}

fn request_pdf_pages(
    preview: &mut PreviewController,
    loader: &PdfPageLoader,
    surface: PdfSurface,
    project_root: PathBuf,
) -> Result<(), String> {
    let Some(key) = preview.raster_request_key() else {
        return Ok(());
    };
    let Some(pdf) = preview.content.pdf().cloned() else {
        return Ok(());
    };
    loader.request(surface, key, pdf, project_root)?;
    preview.record_page_request(key);
    Ok(())
}

fn make_preview_texture(
    context: &egui::Context,
    owner: tiptoptyp_core::document::WindowSessionId,
    key: ArtifactKey,
    index: usize,
    page: PreviewPage,
    dark: bool,
) -> PreviewTexture {
    let PreviewPage { size, rgba, links } = page;
    let request = RasterPageRequestKey {
        artifact: key,
        first: index,
        last: index,
        dpi: crate::pdf::PREVIEW_DPI as u32,
        appearance_revision: 1,
    };
    let resident = make_preview_resident(
        context,
        owner,
        PreviewResidentInput {
            request,
            index,
            size,
            rgba,
            dark,
            visible: true,
        },
    );
    PreviewTexture {
        size,
        links,
        resident: Some(resident),
    }
}

struct PreviewResidentInput {
    request: RasterPageRequestKey,
    index: usize,
    size: [usize; 2],
    rgba: Vec<u8>,
    dark: bool,
    visible: bool,
}

fn make_preview_resident(
    context: &egui::Context,
    owner: tiptoptyp_core::document::WindowSessionId,
    input: PreviewResidentInput,
) -> ResidentPreviewTexture {
    let PreviewResidentInput {
        request,
        index,
        size,
        rgba,
        dark,
        visible,
    } = input;
    let key = request.page_key(index);
    let rgba: Arc<[u8]> = rgba.into();
    let pixels = if dark {
        dark_preview_rgba(&rgba)
    } else {
        rgba.to_vec()
    };
    let texture = context.load_texture(
        format!(
            "preview-{}-{}-{index}-{}-{}",
            key.artifact.revision, key.artifact.generation, key.dpi, key.appearance_revision
        ),
        preview_color_image(size, &pixels),
        TextureOptions::LINEAR,
    );
    let lease = crate::pdf_residency::admit(
        owner,
        rgba.len(),
        pixels.len(),
        crate::worker::RepaintTarget::current(context),
        visible,
    );
    ResidentPreviewTexture {
        key,
        raster_size: size,
        rgba,
        texture,
        lease,
    }
}

fn preview_color_image(size: [usize; 2], rgba: &[u8]) -> ColorImage {
    ColorImage::from_rgba_unmultiplied(size, rgba)
}

fn take_ready_export(
    pending: &mut Option<PendingExport>,
    document_epoch: u64,
    document_revision: u64,
    artifact_key: Option<ArtifactKey>,
) -> Option<PendingExport> {
    let export = pending.as_ref()?;
    if export.document_epoch != document_epoch {
        pending.take();
        return None;
    }
    let ready = artifact_key.is_some_and(|key| {
        key.revision == document_revision
            && export
                .after_artifact_generation
                .is_none_or(|generation| key.generation > generation)
    });
    if ready { pending.take() } else { None }
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
        let fill = theme::palette(ui.ctx()).active_row;
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
        let palette = theme::palette(ui.ctx());
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

fn completion_requested_after_events(events: &[egui::Event]) -> bool {
    events.iter().any(|event| match event {
        egui::Event::Text(text) => text.chars().last().is_some_and(|character| {
            character.is_alphanumeric()
                || matches!(
                    character,
                    '_' | '-' | '.' | '#' | '@' | ':' | '/' | '\\' | ' ' | '"'
                )
        }),
        egui::Event::Key {
            key: egui::Key::Backspace | egui::Key::Delete,
            pressed: true,
            modifiers,
            ..
        } => !modifiers.command && !modifiers.ctrl && !modifiers.alt,
        _ => false,
    })
}

#[allow(clippy::too_many_arguments)]
fn completion_response_matches(
    pending: &EditorCompletionState,
    generation: Generation,
    uri: &str,
    version: i32,
    request_token: u64,
    active_generation: Option<Generation>,
    active_uri: Option<&str>,
    active_version: i32,
) -> bool {
    pending.generation == generation
        && pending.uri == uri
        && pending.version == version
        && pending.request_token == request_token
        && active_generation == Some(generation)
        && active_uri == Some(uri)
        && active_version == version
}

fn editor_attention_progress(elapsed: Duration) -> f32 {
    (elapsed.as_secs_f32() / METRICS.motion.editor_attention.as_secs_f32()).clamp(0.0, 1.0)
}

fn editor_surface_rect(editor_rect: Rect, viewport_rect: Rect) -> Rect {
    editor_rect.union(viewport_rect)
}

#[derive(Clone)]
struct StickyContextEditorSnapshot {
    galley: Arc<egui::Galley>,
    galley_pos: Pos2,
    line_rows: Vec<Range<usize>>,
    scroll_lines: Vec<StickyContextScrollLine>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StickyContextScrollLine {
    top: f32,
    anchor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StickyContextOverlayGeometry {
    anchor: Pos2,
    width: f32,
    max_height: f32,
}

fn sticky_context_overlay_geometry(
    viewport: Rect,
    _find_overlay: Option<Rect>,
) -> Option<StickyContextOverlayGeometry> {
    if !viewport.is_positive() {
        return None;
    }
    let anchor = viewport.left_top();
    let width = viewport.width();
    let available_height = viewport.bottom() - anchor.y;
    let max_height = available_height.min(viewport.height() * STICKY_CONTEXT_MAX_VIEWPORT_FRACTION);
    (width > 1.0 && max_height > 1.0).then_some(StickyContextOverlayGeometry {
        anchor,
        width,
        max_height,
    })
}

fn sticky_context_jump_target(row: &StickyContextRow, clicked: bool) -> Option<usize> {
    clicked.then_some(row.char_index)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct EditorGutterGeometry {
    line_number_right: f32,
    separator_x: f32,
}

fn editor_gutter_geometry(galley_x: f32) -> EditorGutterGeometry {
    EditorGutterGeometry {
        line_number_right: galley_x - METRICS.editor.line_number_right_gap,
        separator_x: galley_x - METRICS.editor.line_number_separator_gap,
    }
}

fn sticky_context_source_row_bounds(
    snapshot: &StickyContextEditorSnapshot,
    one_based_line: usize,
) -> Option<(f32, f32)> {
    let visual_rows = snapshot.line_rows.get(one_based_line.checked_sub(1)?)?;
    let first = snapshot.galley.rows.get(visual_rows.start)?.rect();
    let last = snapshot
        .galley
        .rows
        .get(visual_rows.end.checked_sub(1)?)?
        .rect();
    (last.bottom() > first.top()).then_some((first.top(), last.bottom()))
}

struct StickyContextRenderRow<'a> {
    row: &'a StickyContextRow,
    source_top: f32,
    height: f32,
}

fn sticky_context_push_offset(boundary_top: f32, group_top: f32, group_height: f32) -> f32 {
    (boundary_top - (group_top + group_height)).min(0.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StickyContextMotionRow {
    end_line: usize,
    height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StickyContextMotion {
    offset: f32,
    clip_top: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct StickyContextMotionLayout {
    rows: Vec<StickyContextMotion>,
    visible_bottom: f32,
}

fn sticky_context_motion_layout(
    rows: &[StickyContextMotionRow],
    overlay_top: f32,
    mut boundary_top: impl FnMut(usize) -> Option<f32>,
) -> StickyContextMotionLayout {
    let mut motion = vec![
        StickyContextMotion {
            offset: 0.0,
            clip_top: overlay_top,
        };
        rows.len()
    ];
    let mut destination_top = overlay_top;
    let mut visible_bottom = overlay_top;
    let mut start = 0;
    while start < rows.len() {
        let end_line = rows[start].end_line;
        let mut end = start + 1;
        while end < rows.len() && rows[end].end_line == end_line {
            end += 1;
        }
        let height = rows[start..end].iter().map(|row| row.height).sum::<f32>();
        let offset = boundary_top(end_line)
            .map(|boundary| sticky_context_push_offset(boundary, destination_top, height))
            .unwrap_or(0.0);
        motion[start..end].fill(StickyContextMotion {
            offset,
            // A context ending on its own moves underneath all preceding
            // sticky rows. Rows sharing an ending boundary form one cohort,
            // retain their relative positions, and leave together.
            clip_top: destination_top,
        });
        visible_bottom = visible_bottom.max(destination_top + height + offset);
        destination_top += height;
        start = end;
    }
    StickyContextMotionLayout {
        rows: motion,
        visible_bottom,
    }
}

fn sticky_context_row_reached_boundary(
    source_top: f32,
    overlay_top: f32,
    preceding_sticky_height: f32,
) -> bool {
    source_top <= overlay_top + preceding_sticky_height
}

fn sticky_context_visible_rows<'a>(
    rows: &'a [StickyContextRow],
    snapshot: &StickyContextEditorSnapshot,
    overlay_top: f32,
    max_height: f32,
) -> Vec<StickyContextRenderRow<'a>> {
    let mut visible = Vec::new();
    let mut height = 0.0;
    for row in rows {
        let Some((source_top, source_bottom)) =
            sticky_context_source_row_bounds(snapshot, row.line)
        else {
            continue;
        };
        let row_height = source_bottom - source_top;
        let source_screen_top = source_top + snapshot.galley_pos.y;
        if !sticky_context_row_reached_boundary(source_screen_top, overlay_top, height) {
            break;
        }
        if height + row_height > max_height {
            break;
        }
        visible.push(StickyContextRenderRow {
            row,
            source_top,
            height: row_height,
        });
        height += row_height;
    }
    visible
}

fn sticky_context_stacked_scroll_anchor(
    viewport_top: f32,
    max_height: f32,
    mut anchor_at_boundary: impl FnMut(f32) -> Option<usize>,
    mut stack_at_anchor: impl FnMut(usize) -> StickyContextStackProbe,
) -> Option<usize> {
    let mut boundary = viewport_top;
    let mut retained = None;
    for _ in 0..STICKY_CONTEXT_STACK_RESOLUTION_LIMIT {
        let anchor = anchor_at_boundary(boundary)?;
        let mut candidate = stack_at_anchor(anchor);
        candidate.height = candidate.height.clamp(0.0, max_height);
        if retained
            .as_ref()
            .is_none_or(|(_, current)| sticky_context_probe_supersedes(current, &candidate))
        {
            retained = Some((anchor, candidate));
        }
        // Probing at the bottom of the accumulated stack can land just after
        // a short declaration. Keep a strict-prefix result from erasing the
        // context already found, while still admitting a divergent sibling.
        let (_, retained_probe) = retained.as_ref()?;
        let next_boundary = boundary.max(viewport_top + retained_probe.height);
        if (next_boundary - boundary).abs() <= 0.5 {
            return retained.map(|(anchor, _)| anchor);
        }
        boundary = next_boundary;
    }
    retained.map(|(anchor, _)| anchor)
}

#[derive(Debug, Clone, PartialEq)]
struct StickyContextStackProbe {
    /// Identity of each visible row, ordered outermost to innermost.
    signature: Vec<(usize, usize)>,
    height: f32,
}

fn sticky_context_probe_supersedes(
    current: &StickyContextStackProbe,
    candidate: &StickyContextStackProbe,
) -> bool {
    let candidate_is_strict_prefix = candidate.signature.len() < current.signature.len()
        && current.signature[..candidate.signature.len()] == candidate.signature;
    !candidate_is_strict_prefix
}

fn sticky_context_rows_for_snapshot(
    query: &StickyContextQuery<'_>,
    snapshot: &StickyContextEditorSnapshot,
    viewport_top: f32,
    max_height: f32,
) -> Option<Vec<StickyContextRow>> {
    let mut rows_by_anchor = BTreeMap::new();
    let anchor = sticky_context_stacked_scroll_anchor(
        viewport_top,
        max_height,
        |boundary| sticky_context_scroll_anchor(&snapshot.scroll_lines, boundary),
        |anchor| {
            let rows = rows_by_anchor
                .entry(anchor)
                .or_insert_with(|| query.rows(anchor));
            let visible = sticky_context_visible_rows(rows, snapshot, viewport_top, max_height);
            StickyContextStackProbe {
                signature: visible
                    .iter()
                    .map(|row| (row.row.line, row.row.char_index))
                    .collect(),
                height: visible.iter().map(|row| row.height).sum(),
            }
        },
    )?;
    Some(
        rows_by_anchor
            .remove(&anchor)
            .unwrap_or_else(|| query.rows(anchor)),
    )
}

fn sticky_context_shadow(dark_mode: bool) -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 2],
        blur: 6,
        spread: 0,
        color: Color32::from_black_alpha(if dark_mode { 40 } else { 24 }),
    }
}

fn sticky_context_shadow_clip(overlay: Rect, viewport: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(viewport.left(), overlay.bottom()),
        viewport.right_bottom(),
    )
}

fn sticky_context_opaque_cover(overlay: Rect, viewport: Rect) -> Rect {
    Rect::from_min_max(
        overlay.min,
        Pos2::new(
            overlay.right(),
            (overlay.bottom() + STICKY_CONTEXT_BOTTOM_COVER).min(viewport.bottom()),
        ),
    )
}

fn show_sticky_context_overlay(
    context: &egui::Context,
    viewport: Rect,
    find_overlay: Option<Rect>,
    rows: &[StickyContextRow],
    snapshot: &StickyContextEditorSnapshot,
    line_numbers: bool,
) -> Option<usize> {
    let geometry = sticky_context_overlay_geometry(viewport, find_overlay)?;
    if rows.is_empty() {
        return None;
    }

    let visible_rows =
        sticky_context_visible_rows(rows, snapshot, geometry.anchor.y, geometry.max_height);
    if visible_rows.is_empty() {
        return None;
    }
    let motion_rows = visible_rows
        .iter()
        .map(|row| StickyContextMotionRow {
            end_line: row.row.end_line,
            height: row.height,
        })
        .collect::<Vec<_>>();
    let motion = sticky_context_motion_layout(&motion_rows, geometry.anchor.y, |end_line| {
        sticky_context_source_row_bounds(snapshot, end_line)
            .map(|(top, _)| top + snapshot.galley_pos.y)
    });
    let overlay_height = motion.visible_bottom - geometry.anchor.y;
    if overlay_height <= 0.0 {
        return None;
    }

    let mut jump_target = None;
    egui::Area::new(viewport_scoped_id(context, "sticky-context-overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(geometry.anchor)
        .fade_in(false)
        .constrain_to(viewport)
        .show(context, |ui| {
            ui.set_min_size(Vec2::new(geometry.width, overlay_height));
            let overlay =
                Rect::from_min_size(geometry.anchor, Vec2::new(geometry.width, overlay_height));
            let shadow_clip = sticky_context_shadow_clip(overlay, viewport);
            ui.painter()
                .with_clip_rect(shadow_clip)
                .add(sticky_context_shadow(ui.visuals().dark_mode).as_shape(overlay, 0));
            ui.painter().rect_filled(
                sticky_context_opaque_cover(overlay, viewport),
                0.0,
                ui.visuals().text_edit_bg_color(),
            );

            let gutter = editor_gutter_geometry(snapshot.galley_pos.x);
            if line_numbers {
                ui.painter().line_segment(
                    [
                        Pos2::new(gutter.separator_x, overlay.top()),
                        Pos2::new(gutter.separator_x, overlay.bottom()),
                    ],
                    Stroke::new(
                        METRICS.editor.line_number_separator_width,
                        ui.visuals().widgets.noninteractive.bg_stroke.color,
                    ),
                );
            }

            let mut destination_top = overlay.top();
            for (visible, motion) in visible_rows.into_iter().zip(motion.rows) {
                let row = visible.row;
                let row_rect = Rect::from_min_size(
                    Pos2::new(overlay.left(), destination_top + motion.offset),
                    Vec2::new(overlay.width(), visible.height),
                );
                let visible_rect = row_rect.intersect(Rect::from_min_max(
                    Pos2::new(overlay.left(), motion.clip_top),
                    overlay.right_bottom(),
                ));
                if visible_rect.is_positive() {
                    let response = ui.interact(
                        visible_rect,
                        ui.id()
                            .with(("sticky-context-row", row.line, row.char_index)),
                        Sense::click(),
                    );
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }

                    let painter = ui
                        .painter()
                        .with_clip_rect(visible_rect.intersect(viewport));
                    painter.galley(
                        Pos2::new(snapshot.galley_pos.x, row_rect.top() - visible.source_top),
                        Arc::clone(&snapshot.galley),
                        ui.visuals().text_color(),
                    );
                    if line_numbers {
                        let number = painter.layout_no_wrap(
                            row.line.to_string(),
                            theme::editor_font(),
                            ui.visuals().weak_text_color(),
                        );
                        let source_row =
                            &snapshot.galley.rows[snapshot.line_rows[row.line - 1].start];
                        painter.galley(
                            line_number_position(
                                gutter.line_number_right,
                                row_rect.top(),
                                source_row,
                                &number,
                            ),
                            number,
                            ui.visuals().weak_text_color(),
                        );
                    }
                    if let Some(target) = sticky_context_jump_target(row, response.clicked()) {
                        jump_target = Some(target);
                    }
                }
                destination_top += visible.height;
            }
            ui.painter().line_segment(
                [overlay.left_bottom(), overlay.right_bottom()],
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
        });
    jump_target
}

fn editor_char_range_rect(output: &egui::text_edit::TextEditOutput, range: &Range<usize>) -> Rect {
    let start = output
        .galley
        .pos_from_cursor(CCursor::new(range.start))
        .translate(output.galley_pos.to_vec2());
    let end = output
        .galley
        .pos_from_cursor(CCursor::new(range.end))
        .translate(output.galley_pos.to_vec2());
    editor_range_rect(output.response.rect, start, end)
}

fn editor_range_rect(editor: Rect, start: Rect, end: Rect) -> Rect {
    let same_row = (start.top() - end.top()).abs() <= 0.5;
    let rect = if same_row {
        Rect::from_min_max(
            start.left_top(),
            Pos2::new(end.left().max(start.left() + 1.0), start.bottom()),
        )
    } else {
        // Multiline literals are unusual, but keeping the entire intervening
        // row width hoverable is less surprising than two disconnected slivers.
        Rect::from_min_max(
            Pos2::new(editor.left(), start.top()),
            Pos2::new(editor.right(), end.bottom().max(start.bottom())),
        )
    };
    rect.intersect(editor)
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
        if output.galley.rows[rows.start].size.y == 0.0 {
            continue;
        }
        let color = diagnostic_color(diagnostic.severity, ui.ctx());
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
        if hover_opacity(&response, diagnostic_hover_timing_id(&response.ctx)).is_some()
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
            });
        }
    }
    hovered_diagnostic
}

fn paint_line_numbers(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    line_rows: &[Range<usize>],
    font: egui::FontId,
) {
    let painter = ui.painter();
    let gutter = editor_gutter_geometry(output.galley_pos.x);
    let separator = Stroke::new(
        METRICS.editor.line_number_separator_width,
        ui.visuals().widgets.noninteractive.bg_stroke.color,
    );
    painter.line_segment(
        [
            Pos2::new(gutter.separator_x, output.response.rect.top()),
            Pos2::new(gutter.separator_x, output.response.rect.bottom()),
        ],
        separator,
    );
    let color = ui.visuals().weak_text_color();
    for (line, rows) in line_rows.iter().enumerate() {
        let Some(row) = output.galley.rows.get(rows.start) else {
            continue;
        };
        let row_rect = row.rect().translate(output.galley_pos.to_vec2());
        if row.size.y == 0.0 || !ui.clip_rect().intersects(row_rect) {
            continue;
        }
        let number = painter.layout_no_wrap((line + 1).to_string(), font.clone(), color);
        painter.galley(
            line_number_position(gutter.line_number_right, row_rect.top(), row, &number),
            number,
            color,
        );
    }
}

fn line_number_position(
    right: f32,
    top: f32,
    source: &egui::epaint::text::PlacedRow,
    number: &egui::Galley,
) -> Pos2 {
    let number_baseline = number.rows[0].pos.y + number.rows[0].glyphs[0].pos.y;
    // Source rows can be taller because of fallback glyphs or line spacing.
    // Align baselines, not bounding boxes. Empty rows retain their own height.
    let baseline = source.glyphs.first().map_or_else(
        || (source.size.y - number.size().y).max(0.0) * 0.5 + number_baseline,
        |glyph| glyph.pos.y,
    );
    Pos2::new(right - number.size().x, top + baseline - number_baseline)
}

fn paint_fold_controls(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    line_rows: &[Range<usize>],
    folding: &crate::folding::Folding,
    git_gutter: bool,
) -> Option<crate::folding::FoldRegion> {
    let left = output.response.rect.left()
        + if git_gutter {
            f32::from(crate::git::editor::GUTTER_WIDTH)
        } else {
            0.0
        };
    for region in &folding.regions {
        let Some(rows) = line_rows.get(region.line) else {
            continue;
        };
        let row = &output.galley.rows[rows.start];
        let rect = row.rect().translate(output.galley_pos.to_vec2());
        if row.size.y == 0.0 || !ui.clip_rect().intersects(rect) {
            continue;
        }
        let hit = Rect::from_min_max(
            Pos2::new(left, rect.top()),
            Pos2::new(output.galley_pos.x - 1.0, rect.bottom()),
        );
        let collapsed = folding.is_collapsed(region.line);
        let response = ui.interact(
            hit,
            output.response.id.with(("fold", region.line)),
            Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                format!(
                    "{} line {}",
                    if collapsed { "Expand" } else { "Collapse" },
                    region.line + 1
                ),
            )
        });
        let center = Pos2::new(left + 4.0, rect.center().y);
        let points = if collapsed {
            vec![
                center + Vec2::new(-3.0, -5.0),
                center + Vec2::new(3.0, 0.0),
                center + Vec2::new(-3.0, 5.0),
            ]
        } else {
            vec![
                center + Vec2::new(-4.0, -3.0),
                center + Vec2::new(0.0, 3.0),
                center + Vec2::new(4.0, -3.0),
            ]
        };
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            ui.visuals().text_color(),
            Stroke::NONE,
        ));
        if response.clicked() {
            return Some(region.clone());
        }
    }
    None
}

fn paint_fold_markers(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    line_rows: &[Range<usize>],
    folding: &crate::folding::Folding,
    marker: &Arc<egui::Galley>,
    marker_width: f32,
) -> Option<crate::folding::FoldRegion> {
    for region in folding
        .regions
        .iter()
        .filter(|r| folding.is_collapsed(r.line))
    {
        let Some(rows) = line_rows.get(region.line) else {
            continue;
        };
        let row = &output.galley.rows[rows.end - 1];
        let rect = row.rect().translate(output.galley_pos.to_vec2());
        if row.size.y == 0.0 || !ui.clip_rect().intersects(rect) {
            continue;
        }
        let hit = Rect::from_min_max(
            Pos2::new(rect.right() - marker_width, rect.top()),
            rect.right_bottom(),
        );
        let response = ui.interact(
            hit,
            output.response.id.with(("fold-marker", region.line)),
            Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                format!("Expand folded line {}", region.line + 1),
            )
        });
        ui.painter().galley(
            hit.left_top() + Vec2::new(4.0, 0.0),
            Arc::clone(marker),
            ui.visuals().text_color(),
        );
        if response.clicked() {
            return Some(region.clone());
        }
    }
    None
}

#[cfg(test)]
fn logical_line_count(source: &str) -> usize {
    source.bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn editor_gutter_width(line_number_width: Option<f32>, git_gutter: bool) -> i8 {
    let git_width = if git_gutter {
        crate::git::editor::GUTTER_WIDTH
    } else {
        0
    };
    let maximum_number_width = i8::MAX - crate::git::editor::GUTTER_WIDTH;
    let number_width = line_number_width.map_or(METRICS.editor.gutter_disabled_width, |width| {
        let width = if width.is_finite() {
            width.max(0.0)
        } else {
            0.0
        };
        (width + METRICS.editor.fold_lane_width + METRICS.editor.line_number_right_gap)
            .ceil()
            .min(f32::from(maximum_number_width)) as i8
    });
    number_width + git_width
}

fn line_number_column_width(ui: &egui::Ui, line_count: usize, font: &egui::FontId) -> f32 {
    // Also works with user-selected proportional code fonts. Ten cached glyph
    // metrics, no string allocation or document-sized layout.
    let digit_width = ui.fonts_mut(|fonts| {
        ('0'..='9')
            .map(|digit| fonts.glyph_width(font, digit))
            .fold(0.0, f32::max)
    });
    digit_width * (line_count.max(1).ilog10() + 1) as f32
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

/// Build a compact logical-line index over the laid-out editor rows.
///
/// Character counts include an implicit newline where the galley row ends a
/// source line. Each anchor is the final character before that newline, so a
/// scope beginning on a line is visible to the parser as soon as the line
/// reaches its sticky boundary. Wrapped logical lines retain the top of their
/// first visual row.
fn sticky_context_scroll_lines(
    logical_lines: &[Range<usize>],
    mut visual_row_top: impl FnMut(usize) -> Option<f32>,
    mut visual_row_char_count: impl FnMut(usize) -> Option<usize>,
    mut visual_row_ends_with_newline: impl FnMut(usize) -> Option<bool>,
) -> Option<Vec<StickyContextScrollLine>> {
    let mut lines = Vec::with_capacity(logical_lines.len());
    let mut line_start = 0usize;
    for visual_rows in logical_lines {
        let top = visual_row_top(visual_rows.start)?;
        let line_char_count = visual_rows.clone().try_fold(0usize, |count, row| {
            visual_row_char_count(row).map(|row_count| count + row_count)
        })?;
        let newline_chars = usize::from(visual_row_ends_with_newline(
            visual_rows.end.checked_sub(1)?,
        )?);
        lines.push(StickyContextScrollLine {
            top,
            anchor: line_start + line_char_count.saturating_sub(newline_chars + 1),
        });
        line_start += line_char_count;
    }
    Some(lines)
}

/// Return the representative source character for the last logical line whose
/// first visual row has reached the requested sticky boundary.
fn sticky_context_scroll_anchor(lines: &[StickyContextScrollLine], boundary: f32) -> Option<usize> {
    let index = lines
        .partition_point(|line| line.top <= boundary)
        .checked_sub(1)?;
    Some(lines.get(index)?.anchor)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileDropTarget {
    Editor,
    Folder(PathBuf),
}

fn offer_file_drop_target(ui: &egui::Ui, rect: Rect, target: FileDropTarget) {
    if ui
        .ctx()
        .pointer_latest_pos()
        .is_some_and(|pos| rect.intersect(ui.clip_rect()).contains(pos))
    {
        let id = viewport_scoped_id(ui.ctx(), "file-drop-target");
        ui.ctx().data_mut(|data| data.insert_temp(id, target));
    }
}

fn offer_folder_row_drop(ui: &egui::Ui, row: Rect, directory: &Path) {
    let rect = Rect::from_min_max(
        Pos2::new(ui.clip_rect().left(), row.top()),
        Pos2::new(ui.clip_rect().right(), row.bottom()),
    );
    offer_file_drop_target(ui, rect, FileDropTarget::Folder(directory.to_path_buf()));
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

fn closed_eye_icon_geometry(rect: Rect) -> ([Pos2; 4], [[Pos2; 2]; 3]) {
    // Center the visible lid + lashes, not the (otherwise invisible) full eye.
    let shift = rect.height() * (0.34 * 0.75 + 0.16) * 0.5;
    let lid = eye_icon_geometry(rect.translate(Vec2::new(0.0, -shift))).lower;
    let lashes = [0.2_f32, 0.5, 0.8].map(|t| {
        let s = 1.0 - t;
        let root = (lid[0].to_vec2() * s.powi(3)
            + lid[1].to_vec2() * (3.0 * s * s * t)
            + lid[2].to_vec2() * (3.0 * s * t * t)
            + lid[3].to_vec2() * t.powi(3))
        .to_pos2();
        [
            root,
            root + Vec2::new((t - 0.5) * rect.width() * 0.35, rect.height() * 0.16),
        ]
    });
    (lid, lashes)
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
        UiIcon::EyeClosed => {
            let (lid, lashes) = closed_eye_icon_geometry(rect);
            painter.add(egui::Shape::CubicBezier(
                egui::epaint::CubicBezierShape::from_points_stroke(
                    lid,
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ),
            ));
            for lash in lashes {
                painter.line_segment(lash, stroke);
            }
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
    native_rect: NativeRect,
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
        format_rect(Rect::from_min_max(
            Pos2::new(native_rect.left(), native_rect.top()),
            Pos2::new(native_rect.right(), native_rect.bottom())
        )),
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

fn egui_rect_to_native(context: &egui::Context, rect: Rect) -> Option<NativeRect> {
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

fn scale_rect_from_egui_to_native(rect: Rect, viewport: Rect, scale: f32) -> Option<NativeRect> {
    ViewportTransform::new([viewport.min.x, viewport.min.y], scale)?.to_native(EguiRect::new(
        [rect.min.x, rect.min.y],
        [rect.max.x, rect.max.y],
    )?)
}

fn show_preview_transition(ui: &mut egui::Ui, waiting_for_focus: bool) {
    show_centered_preview_message(
        ui,
        if waiting_for_focus {
            "Activate the document window to resume preview"
        } else {
            "Building preview…"
        },
        !waiting_for_focus,
    );
}

fn preview_message_rects(
    bounds: Rect,
    text_size: Vec2,
    spinner_size: f32,
    gap: f32,
) -> (Rect, Rect) {
    let gap = if spinner_size > 0.0 { gap } else { 0.0 };
    let size = Vec2::new(
        spinner_size + gap + text_size.x,
        spinner_size.max(text_size.y),
    );
    let group = egui::Align2::CENTER_CENTER.align_size_within_rect(size, bounds);
    let spinner = Rect::from_center_size(
        Pos2::new(group.left() + spinner_size * 0.5, group.center().y),
        Vec2::splat(spinner_size),
    );
    let text = Rect::from_center_size(
        Pos2::new(group.right() - text_size.x * 0.5, group.center().y),
        text_size,
    );
    (spinner, text)
}

fn show_centered_preview_message(ui: &mut egui::Ui, message: &str, spinning: bool) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    ui.painter().rect_filled(rect, 0.0, preview_background(ui));
    let spinner_size = if spinning {
        ui.spacing().interact_size.y
    } else {
        0.0
    };
    let gap = if spinning {
        ui.spacing().item_spacing.x
    } else {
        0.0
    };
    let text = ui.painter().layout(
        message.to_owned(),
        theme::supporting_font(),
        ui.visuals().weak_text_color(),
        (rect.width() - spinner_size - gap).max(1.0),
    );
    let (spinner_rect, text_rect) = preview_message_rects(rect, text.size(), spinner_size, gap);
    if spinning {
        ui.put(spinner_rect, egui::Spinner::new().size(spinner_size));
    }
    ui.put(text_rect, egui::Label::new(text));
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
        (path.is_file()
            && path.starts_with(&canonical_root)
            && crate::document::supports_path(&path))
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
    let child = result
        .map_err(|error| format!("Could not open the link in the system browser: {error}"))?;
    let status = crate::process::wait(child, Duration::from_secs(15), || Ok(()))
        .map_err(|error| format!("Browser launcher failed: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Browser launcher exited with {status}"))
    }
}

fn external_link_opened_notice(target: &str) -> Notice {
    Notice {
        message: format!("Opened {target} in the system browser"),
        kind: NoticeKind::Success,
    }
}

fn diagnostic_color(severity: DiagnosticSeverity, context: &egui::Context) -> Color32 {
    match severity {
        DiagnosticSeverity::Error => error_color(context),
        DiagnosticSeverity::Warning => warning_color(context),
        DiagnosticSeverity::Help | DiagnosticSeverity::Note => info_color(context),
        DiagnosticSeverity::Unknown => neutral_color(context),
    }
}

fn error_color(context: &egui::Context) -> Color32 {
    theme::palette(context).error
}

fn warning_color(context: &egui::Context) -> Color32 {
    theme::palette(context).warning
}

fn info_color(context: &egui::Context) -> Color32 {
    theme::palette(context).info
}

fn success_color(context: &egui::Context) -> Color32 {
    theme::palette(context).success
}

fn neutral_color(context: &egui::Context) -> Color32 {
    theme::palette(context).neutral
}

fn notice_color(kind: NoticeKind, context: &egui::Context) -> Color32 {
    match kind {
        NoticeKind::Info => info_color(context),
        NoticeKind::Success => success_color(context),
        NoticeKind::Error => error_color(context),
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

fn preview_timing_label(kind: DocumentKind, elapsed: Duration) -> Option<String> {
    (kind.is_typst() && !elapsed.is_zero()).then(|| format!("{} ms", elapsed.as_millis().max(1)))
}

fn completed_action_label(action: &str, name: &str, elapsed: Duration) -> String {
    format!("{action} {name} in {} ms", elapsed.as_millis().max(1))
}

fn tinymist_status_matches_entry(reported: &str, entry: &Path, root: &Path) -> bool {
    if reported.is_empty() {
        return false;
    }
    let reported = reported.replace('\\', "/");
    reported == entry.to_string_lossy().replace('\\', "/")
        || entry.strip_prefix(root).ok().is_some_and(|relative| {
            reported == format!("/{}", relative.to_string_lossy().replace('\\', "/"))
        })
}

fn app_popup_blocked_by_root_overlay(
    has_modal: bool,
    typst_overrides_visible: bool,
    workspace_chooser_visible: bool,
    has_rename_dialog: bool,
    has_table_editor: bool,
) -> bool {
    has_modal
        || typst_overrides_visible
        || workspace_chooser_visible
        || has_rename_dialog
        || has_table_editor
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
        .width(190.0)
        .height(350.0)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            let query_id = ui.id().with("font-search");
            let mut query = ui
                .ctx()
                .data(|data| data.get_temp::<String>(query_id).unwrap_or_default());
            ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search fonts"));
            ui.ctx()
                .data_mut(|data| data.insert_temp(query_id, query.clone()));
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
            for family in catalog
                .families()
                .iter()
                .filter(|family| crate::completion::fuzzy_score(&family.name, &query).is_some())
            {
                if previous_origin != Some(family.origin) {
                    ui.separator();
                    ui.label(RichText::new(family.origin.label()).strong());
                    previous_origin = Some(family.origin);
                }
                let is_selected = selected_family
                    .is_some_and(|selected| selected.eq_ignore_ascii_case(&family.name))
                    && selected_path.is_some_and(|path| family.contains_path(Path::new(path)));
                let response = ui
                    .selectable_label(is_selected, &family.name)
                    .on_hover_ui(|ui| {
                        ui.label(&family.name);
                        crate::font_preview::show(ui, id, family);
                    });
                if response.clicked()
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
                    success_color(ui.ctx()),
                    format!(
                        "<packaged>/{} {}",
                        resolution.kind.binary_name(),
                        resolution.kind.bundled_version()
                    ),
                )
            } else {
                let color = match resolution.origin {
                    ToolOrigin::Bundled | ToolOrigin::Custom => success_color(ui.ctx()),
                    ToolOrigin::Environment | ToolOrigin::Path => warning_color(ui.ctx()),
                    ToolOrigin::Missing => error_color(ui.ctx()),
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
            let visible_path = tail_elide(&full_path, path_chars);
            let elided = visible_path != full_path;
            let path = ui.add_sized(
                [path_width, METRICS.settings.tool_path_row_height],
                egui::Label::new(
                    RichText::new(visible_path)
                        .size(theme::TYPE.supporting)
                        .monospace(),
                )
                .truncate(),
            );
            if elided
                || path
                    .intrinsic_size()
                    .is_some_and(|size| size.x > path.rect.width())
            {
                settings_hover_text(path, full_path);
            }
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
    let color = warning_color(ui.ctx());
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
        ToolOrigin::Bundled | ToolOrigin::Custom => success_color(ui.ctx()),
        ToolOrigin::Environment | ToolOrigin::Path => warning_color(ui.ctx()),
        ToolOrigin::Missing => error_color(ui.ctx()),
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
        ServiceState::Ready(_) => success_color(ui.ctx()),
        ServiceState::Starting(_) => info_color(ui.ctx()),
        ServiceState::Degraded(_) => warning_color(ui.ctx()),
        ServiceState::Failed(_) => error_color(ui.ctx()),
        ServiceState::Disabled(_) | ServiceState::Unsupported(_) => neutral_color(ui.ctx()),
    };
    show_status_chip(ui, name, state.label(), state.detail(), color);
}

fn show_status_chip(ui: &mut egui::Ui, name: &str, status: &str, detail: &str, color: Color32) {
    let show_detail = settings_status_has_detail(name, status, detail);
    let name = RichText::new(name).strong();
    let status = RichText::new(status)
        .size(theme::TYPE.supporting)
        .strong()
        .color(color);
    let text_size = |text: RichText| {
        egui::WidgetText::from(text)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
            .size()
    };
    let name_size = text_size(name.clone());
    let status_size = text_size(status.clone());
    let frame = theme::status_chip_frame(ui.style());
    let size = Vec2::new(
        name_size.x + ui.spacing().item_spacing.x + status_size.x,
        name_size.y.max(status_size.y),
    ) + frame.total_margin().sum();
    let response = ui
        .allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
            frame
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(name).extend());
                        ui.add(egui::Label::new(status).extend());
                    });
                })
                .response
        })
        .inner;
    if show_detail {
        settings_hover_text(response, detail);
    }
}

fn settings_status_has_detail(name: &str, status: &str, detail: &str) -> bool {
    let detail = detail.trim().trim_end_matches('.');
    !detail.is_empty()
        && !detail.eq_ignore_ascii_case(status)
        && !detail.eq_ignore_ascii_case(name)
        && !detail.eq_ignore_ascii_case(&format!("{name} {status}"))
        && !detail.eq_ignore_ascii_case(&format!("{name}: {status}"))
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
            line: diagnostic.range.start.line.get() as usize + 1,
            column: diagnostic.range.start.character.get() as usize + 1,
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

    let tail_start = text
        .char_indices()
        .rev()
        .nth(max_chars - 2)
        .map_or(0, |(index, _)| index);
    let tail = &text[tail_start..];
    let mut compact = String::with_capacity('…'.len_utf8() + tail.len());
    compact.push('…');
    compact.push_str(tail);
    compact
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn reset_untitled_buffer(document: &mut DocumentSession, autosave_deadline: &mut Option<Instant>) {
    document.replace_unprojected_untitled(DEFAULT_SOURCE);
    *autosave_deadline = None;
}

fn source_editor_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-source-editor")
}

fn owned_input_viewports(current: egui::ViewportId) -> [egui::ViewportId; 12] {
    use crate::child_view::child_viewport_id;
    [
        current,
        child_viewport_id(current, "tiptoptyp-packages"),
        child_viewport_id(current, "tiptoptyp-table-editor-overlay"),
        child_viewport_id(current, "tiptoptyp-rename-overlay"),
        child_viewport_id(current, "tiptoptyp-workspace-chooser"),
        child_viewport_id(current, "tiptoptyp-modal-overlay"),
        child_viewport_id(current, "tiptoptyp-settings"),
        child_viewport_id(current, "tiptoptyp-shortcuts"),
        child_viewport_id(current, "tiptoptyp-typst-overrides"),
        child_viewport_id(current, "asset-hover-overlay"),
        child_viewport_id(current, "tiptoptyp-popup-overlay"),
        child_viewport_id(current, "diagnostic-tooltip-overlay"),
    ]
}

pub(crate) fn owns_focused_input_viewport(context: &egui::Context) -> bool {
    owner_has_focused_viewport(context, context.viewport_id(), true)
}

pub(crate) fn owner_has_focused_viewport(
    context: &egui::Context,
    owner: egui::ViewportId,
    owner_visible: bool,
) -> bool {
    let owned = owned_input_viewports(owner);
    context.input(|input| {
        owned.into_iter().any(|viewport| {
            (owner_visible || viewport != owner)
                && input
                    .raw
                    .viewports
                    .get(&viewport)
                    .is_some_and(|info| info.focused == Some(true) && info.visible() != Some(false))
        })
    })
}

fn focused_input_viewport(context: &egui::Context) -> egui::ViewportId {
    let current = context.viewport_id();
    // Only consider this document window and its own overlays. Every
    // `EditorApp` is rendered once per pass, so scanning all focused viewports
    // would let the primary editor steal shortcuts typed into a secondary
    // document before that document's callback runs.
    let owned = owned_input_viewports(current);
    context.input(|input| {
        owned
            .into_iter()
            .find(|viewport| {
                input
                    .raw
                    .viewports
                    .get(viewport)
                    .is_some_and(|info| info.focused == Some(true))
            })
            .unwrap_or(current)
    })
}

fn clipboard_semantic_kind(event: &egui::Event) -> Option<ClipboardSemanticKind> {
    match event {
        egui::Event::Cut => Some(ClipboardSemanticKind::Cut),
        egui::Event::Copy => Some(ClipboardSemanticKind::Copy),
        egui::Event::Paste(_) => Some(ClipboardSemanticKind::Paste),
        _ => None,
    }
}

/// egui-winit translates the platform's conventional clipboard chords into
/// semantic events before the app sees them. Recover the physical chord so a
/// disabled or rebound shortcut cannot leak through to `TextEdit`'s built-in
/// behavior. A semantic paste produced by our own `RequestPaste` is admitted
/// exactly once without being mistaken for a physical Primary+V press.
fn normalize_text_edit_shortcut_events(
    input: &mut egui::InputState,
    shortcuts: &ShortcutBindings,
    allow_requested_paste: bool,
) -> bool {
    let mut modifiers = input.modifiers;
    let mut requested_paste_admitted = false;
    let mut normalized = Vec::with_capacity(input.events.len());
    for event in std::mem::take(&mut input.events) {
        if let egui::Event::ModifiersChanged(changed) = event {
            modifiers = changed;
            normalized.push(event);
            continue;
        }
        let Some(kind) = clipboard_semantic_kind(&event) else {
            normalized.push(event);
            continue;
        };
        if kind == ClipboardSemanticKind::Paste
            && allow_requested_paste
            && !requested_paste_admitted
        {
            requested_paste_admitted = true;
            normalized.push(event);
            continue;
        }

        match shortcuts.action_for_key_event(kind.key(), modifiers) {
            Some(action) if action == kind.action() => normalized.push(event),
            Some(_) => normalized.push(egui::Event::Key {
                key: kind.key(),
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }),
            None => {}
        }
    }
    input.events = normalized;
    requested_paste_admitted
}

fn take_shortcut_capture_event(input: &mut egui::InputState) -> Option<(egui::Key, Modifiers)> {
    let mut modifiers = input.modifiers;
    for index in 0..input.events.len() {
        match &input.events[index] {
            egui::Event::ModifiersChanged(changed) => modifiers = *changed,
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let result = (*key, *modifiers);
                input.events.remove(index);
                return Some(result);
            }
            event => {
                if let Some(kind) = clipboard_semantic_kind(event) {
                    input.events.remove(index);
                    return Some((kind.key(), modifiers));
                }
            }
        }
    }
    None
}

fn remove_unhandled_text_edit_builtin_events(input: &mut egui::InputState) {
    input.events.retain(|event| {
        !matches!(
            event,
            egui::Event::Key {
                key: egui::Key::A | egui::Key::Y | egui::Key::Z,
                pressed: true,
                modifiers,
                ..
            } if modifiers.command
        )
    });
}

fn standard_text_edit_shortcut(command: AppCommand) -> Option<KeyboardShortcut> {
    let (modifiers, key) = match command {
        AppCommand::Undo => (Modifiers::COMMAND, egui::Key::Z),
        AppCommand::Redo => (Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Z),
        AppCommand::SelectAll => (Modifiers::COMMAND, egui::Key::A),
        _ => return None,
    };
    Some(KeyboardShortcut::new(modifiers, key))
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
                // Keep the comment marker at column zero. This mirrors the
                // editor's line-comment command and avoids shifting the
                // marker when indentation changes.
                at: line,
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

fn shortcut_tooltip(label: &str, shortcuts: &ShortcutBindings, action: ShortcutAction) -> String {
    shortcuts.display(action).map_or_else(
        || label.to_owned(),
        |shortcut| format!("{label} · {shortcut}"),
    )
}

#[derive(Debug, Clone, Copy, Default)]
struct CommandAvailability {
    empty_workspace: bool,
    can_undo: bool,
    can_redo: bool,
    saved_document: bool,
    typst_document: bool,
    typst_preview: bool,
    interactive_preview: bool,
}

impl CommandAvailability {
    fn allows(self, requirement: CommandRequirement) -> bool {
        match requirement {
            CommandRequirement::Always => true,
            CommandRequirement::SavedDocument => self.saved_document,
            CommandRequirement::Undo => self.can_undo,
            CommandRequirement::Redo => self.can_redo,
            CommandRequirement::TypstDocument => self.typst_document,
            CommandRequirement::TypstPreview => self.typst_preview,
            CommandRequirement::InteractivePreview => self.interactive_preview,
        }
    }
}

fn show_command_popup_ui(
    ui: &mut egui::Ui,
    menu: CommandMenu,
    availability: CommandAvailability,
    shortcuts: &ShortcutBindings,
    action: &mut Option<AppPopupAction>,
) {
    ui.spacing_mut().item_spacing.y = theme::SPACE.small;
    let mut previous_section = None;
    for spec in command_specs(menu) {
        if previous_section.is_some_and(|section| section != spec.section) {
            ui.add(egui::Separator::default().spacing(theme::SPACE.control));
        }
        let enabled = if availability.empty_workspace {
            workspace_view::empty_workspace_command(spec.command)
        } else {
            availability.allows(spec.requirement)
        };
        let shortcut = shortcuts.egui(spec.shortcut_action);
        if menu_item_enabled(ui, enabled, spec.title, shortcut).clicked() {
            *action = Some(AppPopupAction::Command(spec.command));
        }
        previous_section = Some(spec.section);
    }
}

fn show_file_popup_ui(
    ui: &mut egui::Ui,
    typst_preview: bool,
    rename_path: Option<&Path>,
    shortcuts: &ShortcutBindings,
    action: &mut Option<AppPopupAction>,
) {
    show_command_popup_ui(
        ui,
        CommandMenu::File,
        CommandAvailability {
            empty_workspace: false,
            can_undo: false,
            can_redo: false,
            saved_document: rename_path.is_some(),
            typst_document: false,
            typst_preview,
            interactive_preview: false,
        },
        shortcuts,
        action,
    );
}

fn show_edit_popup_ui(
    ui: &mut egui::Ui,
    can_undo: bool,
    can_redo: bool,
    can_format: bool,
    can_sync_preview: bool,
    shortcuts: &ShortcutBindings,
    action: &mut Option<AppPopupAction>,
) {
    show_command_popup_ui(
        ui,
        CommandMenu::Edit,
        CommandAvailability {
            empty_workspace: false,
            can_undo,
            can_redo,
            saved_document: false,
            typst_document: can_format,
            typst_preview: false,
            interactive_preview: can_sync_preview,
        },
        shortcuts,
        action,
    );
}

fn show_view_popup_ui(
    ui: &mut egui::Ui,
    typst_document: bool,
    shortcuts: &ShortcutBindings,
    action: &mut Option<AppPopupAction>,
) {
    show_command_popup_ui(
        ui,
        CommandMenu::View,
        CommandAvailability {
            empty_workspace: false,
            can_undo: false,
            can_redo: false,
            saved_document: false,
            typst_document,
            typst_preview: false,
            interactive_preview: false,
        },
        shortcuts,
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
    ui.spacing_mut().item_spacing.y = theme::SPACE.small;
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
        "Preview source"
    } else {
        "Use for preview"
    };
    if menu_item_enabled(ui, is_typst && !preview_selected, preview_label, None).clicked() {
        *action = Some(AppPopupAction::Workspace(
            WorkspaceMenuAction::TogglePreview(path.to_path_buf()),
        ));
    }
    if menu_item_enabled(ui, is_file, "Delete…", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Delete(
            path.to_path_buf(),
        )));
    }
    if menu_item_enabled(ui, is_file, "Rename…", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Rename(
            path.to_path_buf(),
        )));
    }
    ui.add(egui::Separator::default().spacing(theme::SPACE.control));
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
    data.prepare_source(&crate::document::DocumentSnapshot::fixture(
        DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 0, 0),
        source,
    ));
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
    let query_id = ui.id().with("document-font-search");
    let mut query = ui
        .ctx()
        .data(|data| data.get_temp::<String>(query_id).unwrap_or_default());
    ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search fonts"));
    ui.ctx()
        .data_mut(|data| data.insert_temp(query_id, query.clone()));
    for family in families
        .into_iter()
        .filter(|family| crate::completion::fuzzy_score(family, &query).is_some())
    {
        let response = menu_item(ui, family, None);
        let response = response.on_hover_ui(|ui| {
            if let Some(font) = catalog.families().iter().find(|font| font.name == family) {
                crate::font_preview::show(ui, "document-font", font);
            }
        });
        if response.clicked() {
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
    table: Option<&'a EditableTable>,
}

fn show_editor_context_menu_ui(
    ui: &mut egui::Ui,
    options: EditorContextMenuOptions<'_>,
    shortcuts: &ShortcutBindings,
    action: &mut Option<EditorMenuAction>,
) {
    ui.spacing_mut().item_spacing.y = theme::SPACE.small;
    if let Some(link) = options.link {
        if menu_item(ui, "Open Link in Browser", None).clicked() {
            *action = Some(EditorMenuAction::OpenLink(link.to_owned()));
            ui.close();
        }
        ui.add(egui::Separator::default().spacing(theme::SPACE.control));
    }
    if let Some(table) = options.table {
        if menu_item(ui, "Edit Table…", None).clicked() {
            *action = Some(EditorMenuAction::EditTable(table.clone()));
            ui.close();
        }
        ui.add(egui::Separator::default().spacing(theme::SPACE.control));
    }
    if menu_item_enabled(
        ui,
        options.can_undo,
        "Undo",
        shortcuts.egui(ShortcutAction::Undo),
    )
    .clicked()
    {
        *action = Some(EditorMenuAction::Command(AppCommand::Undo));
        ui.close();
    }
    if menu_item_enabled(
        ui,
        options.can_redo,
        "Redo",
        shortcuts.egui(ShortcutAction::Redo),
    )
    .clicked()
    {
        *action = Some(EditorMenuAction::Command(AppCommand::Redo));
        ui.close();
    }
    ui.add(egui::Separator::default().spacing(theme::SPACE.control));
    if menu_item_enabled(
        ui,
        options.has_selection,
        "Cut",
        shortcuts.egui(ShortcutAction::Cut),
    )
    .clicked()
    {
        *action = Some(EditorMenuAction::Command(AppCommand::Cut));
        ui.close();
    }
    if menu_item_enabled(
        ui,
        options.has_selection,
        "Copy",
        shortcuts.egui(ShortcutAction::Copy),
    )
    .clicked()
    {
        *action = Some(EditorMenuAction::Command(AppCommand::Copy));
        ui.close();
    }
    if menu_item(ui, "Paste", shortcuts.egui(ShortcutAction::Paste)).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::Paste));
        ui.close();
    }
    if menu_item(ui, "Select All", shortcuts.egui(ShortcutAction::SelectAll)).clicked() {
        *action = Some(EditorMenuAction::Command(AppCommand::SelectAll));
        ui.close();
    }
    if menu_item(
        ui,
        "Toggle Comment",
        shortcuts.egui(ShortcutAction::ToggleComment),
    )
    .clicked()
    {
        *action = Some(EditorMenuAction::Command(AppCommand::ToggleComment));
        ui.close();
    }
    if menu_item_enabled(
        ui,
        options.can_sync_preview,
        "Reveal in Preview",
        shortcuts.egui(ShortcutAction::SyncPreview),
    )
    .clicked()
    {
        *action = Some(EditorMenuAction::Command(AppCommand::SyncPreview));
        ui.close();
    }
    if options.can_format {
        ui.add(egui::Separator::default().spacing(theme::SPACE.control));
        if menu_item(
            ui,
            "Format Document",
            shortcuts.egui(ShortcutAction::Format),
        )
        .clicked()
        {
            *action = Some(EditorMenuAction::Command(AppCommand::Format));
            ui.close();
        }
    }
}

fn show_table_editor_ui(
    ui: &mut egui::Ui,
    dialog: &mut TableEditorDialog,
    available_width: f32,
    cells_height: f32,
) -> Option<TableEditorUiAction> {
    ui.label(RichText::new("Table editor").strong());
    ui.label(
        RichText::new(
            "Edit static Typst markup cells. Dynamic expressions stay protected and cannot be applied.",
        )
        .size(theme::TYPE.supporting)
        .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(theme::SPACE.small);

    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} × {}",
            dialog.table.row_count(),
            dialog.table.columns
        ));
        if ui.button("Add row").clicked() {
            dialog.table.add_row();
            dialog.focus_first_cell = dialog.table.row_count() == 1;
            changed = true;
        }
        if ui
            .add_enabled(
                dialog.table.row_count() > 0,
                egui::Button::new("Remove last row"),
            )
            .clicked()
        {
            let last = dialog.table.row_count() - 1;
            dialog.table.remove_row(last);
            changed = true;
        }
        if ui.button("Add column").clicked() {
            dialog.table.add_column();
            changed = true;
        }
        if ui
            .add_enabled(
                dialog.table.columns > 1,
                egui::Button::new("Remove last column"),
            )
            .clicked()
        {
            dialog.table.remove_column();
            changed = true;
        }
    });
    ui.separator();

    let cell_width =
        ((available_width - 112.0) / dialog.table.columns.min(4) as f32).clamp(112.0, 220.0);
    let mut remove_row = None;
    egui::ScrollArea::both()
        .id_salt("table-editor-cells")
        .max_height(cells_height)
        .show(ui, |ui| {
            egui::Grid::new(viewport_scoped_id(ui.ctx(), "table-editor-grid"))
                .spacing(Vec2::new(theme::SPACE.small, theme::SPACE.small))
                .striped(true)
                .show(ui, |ui| {
                    ui.label("");
                    for column in 0..dialog.table.columns {
                        ui.label(RichText::new(format!("Column {}", column + 1)).strong());
                    }
                    ui.label("");
                    ui.end_row();

                    for row in 0..dialog.table.row_count() {
                        ui.label(RichText::new(format!("Row {}", row + 1)).strong());
                        for column in 0..dialog.table.columns {
                            let response = ui.add_sized(
                                [cell_width, 56.0],
                                egui::TextEdit::multiline(&mut dialog.table.cells[row][column])
                                    .id_salt(("table-cell", row, column))
                                    .desired_width(cell_width)
                                    .desired_rows(2),
                            );
                            if dialog.focus_first_cell && row == 0 && column == 0 {
                                response.request_focus();
                                dialog.focus_first_cell = false;
                            }
                            changed |= response.changed();
                        }
                        if ui.small_button("Remove").clicked() {
                            remove_row = Some(row);
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(row) = remove_row {
        dialog.table.remove_row(row);
        changed = true;
    }
    if changed {
        dialog.error = None;
    }

    if let Some(error) = &dialog.error {
        ui.add_space(theme::SPACE.small);
        ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
    }
    ui.add_space(theme::SPACE.control);
    let mut action = None;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.button("Apply").clicked() {
            action = Some(TableEditorUiAction::Apply);
        }
        if ui.button("Cancel").clicked() {
            action = Some(TableEditorUiAction::Cancel);
        }
    });
    action
}

fn prepare_table_source_edit(
    source: &str,
    document_key: DocumentKey,
    dialog: &TableEditorDialog,
) -> Result<PreparedTableSourceEdit, String> {
    if document_key != dialog.document_key {
        return Err("The document changed while the table editor was open. Reopen the table to edit the latest source.".to_owned());
    }
    let SourceEdit { range, replacement } = dialog
        .table
        .source_edit()
        .map_err(|error| format!("Cannot apply this table: {error}"))?;
    let byte_range = char_range_to_byte_range(source, range.clone())
        .ok_or_else(|| "The table range is no longer valid".to_owned())?;
    if source.get(byte_range.clone()) != Some(dialog.original_call.as_str()) {
        return Err(
            "The table source changed while the table editor was open. Reopen it to continue."
                .to_owned(),
        );
    }
    let cursor = range.start + replacement.chars().count();
    Ok(PreparedTableSourceEdit {
        byte_range,
        replacement,
        cursor,
    })
}

fn char_range_to_byte_range(source: &str, range: Range<usize>) -> Option<Range<usize>> {
    let start = char_index_to_byte(source, range.start)?;
    let end = char_index_to_byte(source, range.end)?;
    Some(start..end)
}

fn char_index_to_byte(source: &str, index: usize) -> Option<usize> {
    if index == source.chars().count() {
        Some(source.len())
    } else {
        source.char_indices().nth(index).map(|(byte, _)| byte)
    }
}

fn char_range_slice(source: &str, range: Range<usize>) -> Option<&str> {
    source.get(char_range_to_byte_range(source, range)?)
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
        let color = notice_color(entry.kind, ui.ctx());
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

fn settings_heading(ui: &mut egui::Ui, section: SettingsSection) {
    ui.heading(section.title());
}

fn settings_target_anchor(
    ui: &mut egui::Ui,
    target: SettingsTarget,
    pending: &mut Option<SettingsTarget>,
) {
    if take_settings_scroll_target(pending, target) {
        let rect = Rect::from_min_size(
            ui.next_widget_position(),
            Vec2::new(ui.available_width().max(1.0), ui.spacing().interact_size.y),
        );
        ui.scroll_to_rect(rect, Some(Align::Min));
    }
}

fn show_shortcut_editor_contents(
    ui: &mut egui::Ui,
    query: &mut String,
    capture: &mut Option<ShortcutAction>,
    notice: &mut Option<String>,
    pending_settings: &mut Option<AppSettings>,
    settings: &AppSettings,
) {
    let mut edited = pending_settings.clone().unwrap_or_else(|| settings.clone());
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(query)
                .hint_text("Search actions or groups")
                .desired_width(f32::INFINITY),
        );
        if ui.button("Reset all").clicked() {
            edited.shortcut_overrides.reset_all();
            *notice = Some("Restored all default shortcuts".to_owned());
        }
    });
    if let Some(action) = *capture {
        ui.colored_label(
            ui.visuals().selection.stroke.color,
            format!(
                "Press the new shortcut for {} · Backspace disables · Esc cancels",
                action.label()
            ),
        );
    } else if let Some(message) = notice.as_deref() {
        ui.label(RichText::new(message).weak());
    }
    ui.separator();

    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let bindings = edited.effective_shortcuts();
    if !bindings.conflicts().is_empty() {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "{} saved shortcut conflict{} could not be activated",
                bindings.conflicts().len(),
                if bindings.conflicts().len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        );
    }
    egui::ScrollArea::vertical()
        .id_salt("shortcut-editor-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut group = None;
            let mut shown = 0usize;
            for action in ShortcutAction::ALL {
                let haystack =
                    format!("{} {} {}", action.group(), action.label(), action.id()).to_lowercase();
                if !terms.iter().all(|term| haystack.contains(term)) {
                    continue;
                }
                if group != Some(action.group()) {
                    if group.is_some() {
                        ui.separator();
                    }
                    ui.label(RichText::new(action.group()).strong());
                    group = Some(action.group());
                }
                shown += 1;
                ui.horizontal(|ui| {
                    let controls_width = 290.0;
                    let label_width = (ui.available_width() - controls_width).max(100.0);
                    ui.add_sized(
                        [label_width, METRICS.menu.row_height],
                        egui::Label::new(action.label()),
                    );
                    ui.add_sized(
                        [100.0, METRICS.menu.row_height],
                        egui::Label::new(
                            RichText::new(
                                bindings
                                    .display(action)
                                    .unwrap_or_else(|| "Unassigned".to_owned()),
                            )
                            .monospace(),
                        ),
                    );
                    if ui
                        .selectable_label(*capture == Some(action), "Change")
                        .clicked()
                    {
                        *capture = Some(action);
                        *notice = None;
                    }
                    if ui.button("Disable").clicked() {
                        edited.shortcut_overrides.set(action, None);
                        *capture = None;
                        *notice = Some(format!("Disabled {}", action.label()));
                    }
                    if ui
                        .add_enabled(
                            edited.shortcut_overrides.get(action).is_some(),
                            egui::Button::new("Reset"),
                        )
                        .clicked()
                    {
                        edited.shortcut_overrides.reset(action);
                        *notice = Some(format!("Restored {}", action.label()));
                    }
                });
            }
            if shown == 0 {
                ui.label(RichText::new("No shortcut actions found").weak());
            }
        });
    *pending_settings = (edited != *settings).then_some(edited);
}

fn consume_preview_zoom_shortcut(
    input: &mut egui::InputState,
    shortcuts: &ShortcutBindings,
) -> Option<PreviewZoomAction> {
    for (shortcut_action, action) in [
        (ShortcutAction::PreviewZoomIn, PreviewZoomAction::In),
        (ShortcutAction::PreviewZoomOut, PreviewZoomAction::Out),
        (ShortcutAction::PreviewZoomReset, PreviewZoomAction::Reset),
    ] {
        if shortcuts
            .egui(shortcut_action)
            .is_some_and(|shortcut| input.consume_shortcut(&shortcut))
        {
            return Some(action);
        }
    }
    None
}

fn consume_ui_scale_shortcut(
    input: &mut egui::InputState,
    shortcuts: &ShortcutBindings,
) -> Option<i16> {
    if shortcuts
        .egui(ShortcutAction::UiScaleIn)
        .is_some_and(|shortcut| input.consume_shortcut(&shortcut))
    {
        return Some(5);
    }
    shortcuts
        .egui(ShortcutAction::UiScaleOut)
        .is_some_and(|shortcut| input.consume_shortcut(&shortcut))
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

fn save_as_format_handoff(
    saved: bool,
    destination_kind: DocumentKind,
    key: DocumentKey,
) -> Option<DocumentKey> {
    (saved && destination_kind.is_typst()).then_some(key)
}

fn take_ready_format_handoff(
    pending: &mut Option<DocumentKey>,
    current: DocumentKey,
    session_ready: bool,
) -> bool {
    if session_ready && *pending == Some(current) {
        pending.take();
        true
    } else {
        false
    }
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
        initial_path
            .is_none()
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
    (path.is_file() && path.starts_with(&root) && crate::document::supports_path(&path))
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

fn atomic_write(
    path: &Path,
    contents: &[u8],
) -> Result<tiptoptyp::save_transaction::WriteDurability, String> {
    crate::private_workspace::AtomicFileWriter::write(path, contents)
        .map_err(|error| format!("Could not replace {}: {error}", path.display()))
}

fn discover_project_root(source_dir: &Path) -> PathBuf {
    source_dir
        .ancestors()
        .find(|ancestor| ancestor.join("typst.toml").is_file() || ancestor.join(".git").exists())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| source_dir.to_path_buf())
}

fn preserve_workspace_snapshot_for_open(
    snapshot_root: Option<&Path>,
    workspace_root: &Path,
    path: &Path,
) -> bool {
    snapshot_root.is_some_and(|snapshot_root| same_path(snapshot_root, workspace_root))
        && path.starts_with(workspace_root)
}

#[cfg(test)]
mod tests;
