mod editor_view;
mod native_views;
mod settings_view;

use std::{
    borrow::Cow,
    collections::{BTreeMap, VecDeque, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tiptoptyp_core::geometry::{EguiRect, NativeRect, ViewportTransform};
#[cfg(test)]
use tiptoptyp_core::text::LspPosition;
use tiptoptyp_core::text::{LspRange, LspTextEdit, ScalarOffset};
use tiptoptyp_core::text::{
    apply_text_edits, lsp_position_at_scalar, range_to_scalar_range, scalar_position_at,
};

use eframe::egui::{
    self, Align, Color32, ColorImage, KeyboardShortcut, Layout, Modifiers, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, TextureOptions, Vec2,
    text::{CCursor, CCursorRange},
};
use egui_ltreeview::{Action as TreeAction, NodeBuilder, TreeView, TreeViewBuilder, TreeViewState};
use rfd::AsyncFileDialog;

use crate::{
    asset::{AssetLoader, AssetThumbnailLoader, AssetThumbnailResult, LoadedAsset},
    builtin_themes,
    child_view::{ChildViewHost, ChildViewSpec, scoped_child_viewport_id, viewport_scoped_id},
    compiler::{ArtifactKey, CompileEvent, CompileRequest, Compiler, PreviewPage},
    diagnostics::{
        Diagnostic, DiagnosticLocation, DiagnosticSeverity, DiagnosticSource,
        normalize_diagnostics, parse_typst_short_output,
    },
    document::{DocumentKey, DocumentKind, DocumentSession, EditorSnapshot},
    editor_data::{EditorDerivedData, FontArgumentTarget, LineDiagnostic},
    editor_features::{
        EditableTable, PreviewAssetKind, SourceEdit, StickyContextQuery, StickyContextRow,
        editable_table_at, literal_asset_target_at,
    },
    font_catalog::{FontCatalog, FontFamily, ignored_workspace_directory, is_font_path},
    generic_highlight::GenericSyntaxHighlighter,
    highlight::SyntaxHighlighter,
    native_menu::{
        AppCommand, CommandMenu, CommandRequirement, NativeMenuCommandQueue, command_spec,
        command_specs, consume_shortcut,
    },
    package_catalog::{PackageCatalogLoad, PackageRecord, PackageRootKind, PackageRoots},
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
    search::SearchSession,
    settings::{
        AppSettings, ColorThemeChoice, DEFAULT_HOVER_DELAY_MS, DEFAULT_HOVER_FADE_MS,
        DEFAULT_UI_FONT_WEIGHT, DEFAULT_UI_SCALE_PERCENT, DocumentTheme, InterfaceTheme,
        PreviewPreference, SourcePreviewTrigger, ToolMode, ToolPreference,
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
const COMPLETION_POPUP_WIDTH: f32 = 360.0;
const COMPLETION_POPUP_MAX_HEIGHT: f32 = 248.0;
const COMPLETION_ROW_HEIGHT: f32 = 26.0;
const STICKY_CONTEXT_MAX_VIEWPORT_FRACTION: f32 = 0.45;
const STICKY_CONTEXT_STACK_RESOLUTION_LIMIT: usize = 32;
const STICKY_CONTEXT_BOTTOM_COVER: f32 = 1.0;
const STICKY_CONTEXT_SNAPSHOT_SCROLL_OFFSET: f32 = 176.0;
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

/// A capture batch owns its fixture independently of each scene's editor state.
struct SceneDocument {
    source: String,
    path: Option<PathBuf>,
    kind: DocumentKind,
    fingerprint: Option<u64>,
}
impl SceneDocument {
    fn capture(document: &DocumentSession) -> Self {
        Self {
            source: document.source().clone(),
            path: document.path().clone(),
            kind: document.kind(),
            fingerprint: document.disk_fingerprint(),
        }
    }
    fn restore(&self, document: &mut DocumentSession) -> bool {
        if document.source() == &self.source
            && document.saved_source() == &self.source
            && document.path() == &self.path
            && document.kind() == self.kind
            && document.disk_fingerprint() == self.fingerprint
        {
            return false;
        }
        if let Some(path) = &self.path {
            document.replace_loaded(
                self.source.clone(),
                path.clone(),
                self.kind,
                self.fingerprint,
            );
        } else {
            document.replace_untitled(self.source.clone());
        }
        true
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

const STICKY_CONTEXT_SNAPSHOT_SOURCE: &str = r##"#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")
#let accent = rgb("#4f8cff")

= Running todo list
== Active subsection
#let review(
  task,
  state,
) = 2

#let review_checks = (
    "Review the task",
    "Keep the declaration header visible",
    "Compare every sticky-row boundary",
    "Preserve the source indentation",
    "Preserve the syntax colors",
    "Preserve the editor gutter",
    "Keep the caret at the beginning",
    "Scroll through the function body",
    "Check the first stacked row",
    "Check the second stacked row",
    "Check the third stacked row",
    "Check the remaining signature rows",
    "Confirm the lower floating shadow",
    "Continue below the pinned context",
)

#review("the workspace", "progress")

- Review the workspace layout
- Confirm the document entry point
- Check the active preview backend
- Read the compiler diagnostics
- Verify the selected color theme
- Inspect the source editor gutter
- Confirm syntax highlighting
- Check wrapped source lines
- Review the current section
- Update the first task
- Update the second task
- Update the third task
- Re-run the focused tests
- Inspect the test output
- Check the status bar
- Confirm the saved document path
- Review the package catalog
- Inspect installed package versions
- Search the settings controls
- Check the configured shortcuts
- Exercise completion results
- Review the table editor
- Add a table row
- Remove a table column
- Inspect the Explorer outline
- Check the active file styling
- Browse the package directory
- Open the Problems panel
- Select a diagnostic
- Jump to its source line
- Inspect the diagnostic tooltip
- Move through the tooltip bridge
- Verify the tooltip dismissal edge
- Check the asset preview
- Inspect the application icon
- Review external-link handling
- Save the current document
- Save the document under a new name
- Compile the current PDF
- Pause automatic preview updates
- Resume automatic preview updates
- Check the newest preview generation
- Open the File menu
- Open the Edit menu
- Review menu shortcut labels
- Check the native Quit workflow
- Reopen the workspace chooser
- Verify the sticky section reminder
- Scroll farther through the section
- Confirm the heading remains pinned
- Confirm the caret remains at the start
- Compare the gutter alignment
- Compare the source baseline
- Compare the syntax colors
- Check the lower floating shadow
- Review the light theme
- Review the dark theme
- Capture the deterministic scene
- Validate the capture filename
- Finish the visual review
- Recheck the pinned source row
- Confirm the final gutter baseline
- Inspect the floating edge
- Compare the editor background
- Verify the heading token colors
- Check the source text weight
- Confirm the sticky row width
- Review the bottom shadow
- Keep the caret above the viewport
- Complete the sticky-context audit
"##;

const fn source_editor_snapshot_scroll_offset(scene: Option<UiSnapshotScene>) -> Option<f32> {
    match scene {
        Some(UiSnapshotScene::StickyContext) => Some(STICKY_CONTEXT_SNAPSHOT_SCROLL_OFFSET),
        _ => None,
    }
}

fn prepare_sticky_context_snapshot_document(document: &mut DocumentSession) -> bool {
    if document.source() == STICKY_CONTEXT_SNAPSHOT_SOURCE {
        return false;
    }
    document.edit(CCursorRange::one(CCursor::new(0)), |source| {
        *source = STICKY_CONTEXT_SNAPSHOT_SOURCE.to_owned()
    });
    // `show_editor` consumes this flag by clearing TextEdit's undo state and
    // putting its caret at character zero. The forced ScrollArea offset does
    // not move that caret, which makes this scene exercise scroll-derived
    // sticky context rather than the old cursor-derived behavior.
    document.reset_editor_history = true;
    true
}

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

/// The Explorer closes in two render steps so its contents disappear before
/// the resizable panel itself does. Besides avoiding a distracting flash of
/// clipped rows, this keeps the panel's last width available to egui until the
/// blank frame has been painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExplorerPanelPhase {
    Open,
    HideContents,
    Closed,
}

impl ExplorerPanelPhase {
    fn toggle(self) -> Self {
        match self {
            Self::Open => Self::HideContents,
            Self::HideContents | Self::Closed => Self::Open,
        }
    }

    fn panel_visible(self) -> bool {
        !matches!(self, Self::Closed)
    }

    fn contents_visible(self) -> bool {
        matches!(self, Self::Open)
    }

    fn finish_frame(self) -> Self {
        match self {
            Self::HideContents => Self::Closed,
            other => other,
        }
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
    generation: Generation,
    uri: String,
    version: i32,
    request_token: u64,
    cursor: usize,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionApplication {
    source: String,
    cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnippetExpansion {
    text: String,
    cursor: usize,
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

#[derive(Debug, Clone)]
struct AssetHoverCandidate {
    origin: Rect,
    anchor: Pos2,
    placement: TooltipPlacement,
    path: PathBuf,
    kind: DocumentKind,
    opacity: f32,
}

#[derive(Clone)]
enum AssetHoverContent {
    Loading,
    Ready {
        texture: egui::TextureHandle,
        source_size: [usize; 2],
    },
    Error(String),
}

#[derive(Clone)]
struct AssetHoverState {
    origin: Rect,
    anchor: Pos2,
    placement: TooltipPlacement,
    path: PathBuf,
    kind: DocumentKind,
    opacity: f32,
    token: crate::asset::ThumbnailToken,
    content: AssetHoverContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TooltipPlacement {
    Below,
    Right,
}

#[derive(Debug, Clone, Copy)]
struct TooltipFadeState {
    opacity: f32,
    updated_at: f64,
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
    fade: TooltipFadeState,
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
    Delete(PathBuf),
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
        table: Option<EditableTable>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageFilter {
    All,
    Installed,
    Available,
    Updates,
}

impl PackageFilter {
    const ALL: [Self; 4] = [Self::All, Self::Installed, Self::Available, Self::Updates];

    const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Installed => "Installed",
            Self::Available => "Available",
            Self::Updates => "Updates",
        }
    }

    fn allows(self, package: &PackageRecord) -> bool {
        match self {
            Self::All => true,
            Self::Installed => package.is_installed(),
            Self::Available => package.latest_available.is_some(),
            Self::Updates => package.has_update(),
        }
    }
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
    PageTheme,
    WrapLines,
    LineNumbers,
    StickyContextRows,
    AutoSave,
    AutoSaveDelay,
    KeyboardShortcuts,
    InterfaceScale,
    TitleBarMenus,
    UiFont,
    UiFontWeight,
    CodeFont,
    CodeFontWeight,
    PreviewJump,
    HoverDelay,
    HoverFade,
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
    const ALL: [Self; 30] = [
        Self::Appearance,
        Self::TypstSyntax,
        Self::LightTheme,
        Self::DarkTheme,
        Self::InvertColors,
        Self::HueShift,
        Self::PageTheme,
        Self::WrapLines,
        Self::LineNumbers,
        Self::StickyContextRows,
        Self::AutoSave,
        Self::AutoSaveDelay,
        Self::KeyboardShortcuts,
        Self::InterfaceScale,
        Self::TitleBarMenus,
        Self::UiFont,
        Self::UiFontWeight,
        Self::CodeFont,
        Self::CodeFontWeight,
        Self::PreviewJump,
        Self::HoverDelay,
        Self::HoverFade,
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
            Self::PageTheme => "Page",
            Self::WrapLines => "Wrap lines",
            Self::LineNumbers => "Line numbers",
            Self::StickyContextRows => "Sticky context rows",
            Self::AutoSave => "Auto-save",
            Self::AutoSaveDelay => "Auto-save delay",
            Self::KeyboardShortcuts => "Keyboard shortcuts…",
            Self::InterfaceScale => "Interface scale",
            Self::TitleBarMenus => "Show title-bar menus",
            Self::UiFont => "UI font",
            Self::UiFontWeight => "UI font weight",
            Self::CodeFont => "Code font",
            Self::CodeFontWeight => "Code font weight",
            Self::PreviewJump => "Preview jump",
            Self::HoverDelay => "Hover delay",
            Self::HoverFade => "Hover fade",
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
            | Self::PageTheme => SettingsSection::Appearance,
            Self::WrapLines
            | Self::LineNumbers
            | Self::StickyContextRows
            | Self::AutoSave
            | Self::AutoSaveDelay
            | Self::KeyboardShortcuts
            | Self::InterfaceScale
            | Self::TitleBarMenus
            | Self::UiFont
            | Self::UiFontWeight
            | Self::CodeFont
            | Self::CodeFontWeight
            | Self::PreviewJump
            | Self::HoverDelay
            | Self::HoverFade => SettingsSection::Editor,
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
            Self::PageTheme => "document light dark follow interface effective",
            Self::WrapLines => "editor soft wrapping",
            Self::LineNumbers => "editor gutter",
            Self::StickyContextRows => "editor headings scopes sections breadcrumbs",
            Self::AutoSave => "editor autosave automatic save",
            Self::AutoSaveDelay => "editor autosave automatic save milliseconds timing",
            Self::KeyboardShortcuts => "editor keys bindings configurable commands",
            Self::InterfaceScale => "editor ui zoom percent size",
            Self::TitleBarMenus => "editor titlebar file edit view chrome",
            Self::UiFont => "editor interface family system choose",
            Self::UiFontWeight => "editor interface bold variable",
            Self::CodeFont => "editor source monospace family choose",
            Self::CodeFontWeight => "editor source monospace bold variable",
            Self::PreviewJump => "editor source sync click double modifier",
            Self::HoverDelay => "editor tooltip wait milliseconds timing",
            Self::HoverFade => "editor tooltip opacity animation milliseconds timing",
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
    document: DocumentSession,
    process_close_pending: bool,
    highlighter: SyntaxHighlighter,
    generic_highlighter: GenericSyntaxHighlighter,

    compiler: Compiler,
    asset_loader: AssetLoader,
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
    filesystem_phase: ExplorerPanelPhase,
    explorer_query: String,
    problems_visible: bool,
    settings_visible: bool,
    settings_query: String,
    settings_scroll_target: Option<SettingsTarget>,
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
    staged_ui_font_weight: Option<u16>,
    staged_code_font_weight: Option<u16>,
    tool_refresh_requested: bool,
    typst_tool: ToolResolution,
    tinymist_tool: ToolResolution,
    workspace_root: PathBuf,
    git: crate::git::GitPanel,
    git_editor: crate::git::editor::GitEditorState,
    workspace_chooser_visible: bool,
    workspace: Option<WorkspaceTree>,
    workspace_error: Option<String>,
    file_import: crate::worker::ExclusiveJob<String>,
    package_uninstall: crate::worker::ExclusiveJob<String>,
    workspace_scan: LatestJob<WorkspaceSnapshot>,
    next_workspace_refresh: Instant,
    project_index: ProjectIndex,
    project_index_deadline: tiptoptyp_core::scheduling::Debounce<Instant>,
    project_index_job: LatestJob<ProjectIndex>,
    captures: CaptureController,
    snapshot_scene: Option<UiSnapshotScene>,
    snapshot_document: Option<SceneDocument>,
    snapshot_font_file: Option<tempfile::NamedTempFile>,
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
    tooltip_request: Option<TooltipRequest>,
    editor_completion: Option<EditorCompletionState>,
    next_editor_completion_token: u64,
    last_editor_caret: Option<EditorCaretState>,
    manual_format_revision: Option<u64>,
    format_when_tinymist_ready: Option<DocumentKey>,
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
    table_editor: Option<TableEditorDialog>,
    table_editor_had_focus: bool,
    table_editor_suspended: bool,

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
            egui::ViewportId::ROOT,
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
        viewport: egui::ViewportId,
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
        theme::set_imported_palette(
            context,
            Some((active_theme.dark_mode, active_theme.palette)),
        );
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
            theme::syntax_palette_from_semantic(active_theme.palette),
            Some(&active_theme.syntect_theme),
            settings.typst_overrides.for_dark(active_theme.dark_mode),
        ));
        let mut app = Self {
            process_close_pending: false,
            document: DocumentSession::new(
                tiptoptyp_core::document::WindowSessionId::new(viewport.0.value()),
                DEFAULT_SOURCE,
                DocumentKind::Typst,
            ),
            highlighter,
            generic_highlighter,
            compiler: Compiler::new(crate::worker::RepaintTarget::new(context, viewport)),
            asset_loader: AssetLoader::new(crate::worker::RepaintTarget::new(context, viewport)),
            asset_token: Default::default(),
            asset_thumbnail_loader: AssetThumbnailLoader::new(crate::worker::RepaintTarget::new(
                context, viewport,
            )),
            asset_thumbnail_token: Default::default(),
            asset_hover: None,
            pending_asset_page: None,
            compile_deadline: Some(Instant::now()),
            compilation_paused: false,
            preview: PreviewController::new(preview_dark, settings.preview_preference),
            editor_data: EditorDerivedData::default(),
            view_mode: ViewMode::Split,
            filesystem_phase: ExplorerPanelPhase::Open,
            explorer_query: String::new(),
            problems_visible: false,
            settings_visible: false,
            settings_query: String::new(),
            settings_scroll_target: None,
            shortcut_editor_visible: false,
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
            staged_ui_font_weight: None,
            staged_code_font_weight: None,
            tool_refresh_requested: false,
            typst_tool,
            tinymist_tool,
            workspace_root,
            git: crate::git::GitPanel::default(),
            git_editor: crate::git::editor::GitEditorState::default(),
            workspace_chooser_visible: false,
            workspace: None,
            workspace_error: None,
            file_import: Default::default(),
            package_uninstall: Default::default(),
            workspace_scan: LatestJob::default(),
            next_workspace_refresh: Instant::now(),
            project_index: ProjectIndex::default(),
            project_index_deadline: tiptoptyp_core::scheduling::Debounce::at(Instant::now()),
            project_index_job: LatestJob::default(),
            captures,
            snapshot_scene,
            snapshot_document: None,
            snapshot_font_file: None,
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
            format_when_tinymist_ready: None,
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
            table_editor: None,
            table_editor_had_focus: false,
            table_editor_suspended: false,
            pending_tool_picker: None,
            notice: None,
            status_log: VecDeque::new(),
            recorded_status: None,
            recorded_notice: None,
            next_external_file_check: Instant::now(),
            external_file_change_notice: None,
            last_title: String::new(),
            tinymist: TinymistSidecar::new(crate::worker::RepaintTarget::new(context, viewport)),
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

    pub(crate) fn native_command_enabled(&self, command: AppCommand) -> bool {
        match command_spec(command).requirement {
            CommandRequirement::Always => true,
            CommandRequirement::SavedDocument => self.document.path().is_some(),
            // Undo/redo history is viewport-local egui state. Keep these menu
            // items enabled so the active viewport can make the final choice.
            CommandRequirement::Undo | CommandRequirement::Redo => true,
            CommandRequirement::TypstDocument => self.document.kind().is_typst(),
            CommandRequirement::TypstPreview => self.typst_preview_available(),
            CommandRequirement::InteractivePreview => {
                self.document.kind().is_typst() && self.interactive_preview_active()
            }
        }
    }

    pub(crate) fn take_window_request(&mut self) -> Option<EditorWindowRequest> {
        self.pending_window_requests.pop_front()
    }

    pub(crate) fn can_reuse_for_external_open(&self) -> bool {
        self.document.path().is_none() && !self.is_dirty() && !self.document_flow_busy()
    }

    pub(crate) fn open_external_path(&mut self, path: PathBuf) {
        self.queued_open_requests.push_back(path);
    }

    pub(crate) fn window_title(&self) -> String {
        self.title()
    }

    pub(crate) fn document_key(&self) -> DocumentKey {
        self.document.key()
    }
    pub(crate) fn begin_process_close(&mut self) -> bool {
        if self.document_flow_busy() {
            return false;
        }
        self.process_close_pending = true;
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
        if !accepted {
            self.document_workflow.revoke_close();
        }
    }
    pub(crate) fn process_close_pending(&self) -> bool {
        self.process_close_pending
    }

    pub(crate) fn is_dirty_for_close(&self) -> bool {
        self.is_dirty()
    }

    pub(crate) fn close_accepted(&self) -> bool {
        self.document_workflow.may_close(self.document.key())
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
                (self.document.kind().is_typst())
                    .then(|| self.document.path().clone())
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
        typst_preview_available_for(
            self.document.kind(),
            self.designated_preview_path().is_some(),
        )
    }

    fn current_is_preview_document(&self) -> bool {
        match &self.document.path() {
            Some(path) => same_path(path, &self.preview_document_path()),
            None => self.designated_preview_path().is_none() && self.document.kind().is_typst(),
        }
    }

    fn preview_document_source(&self) -> Result<String, String> {
        let path = self.preview_document_path();
        if self.current_is_preview_document() {
            return Ok(self.document.source().clone());
        }
        fs::read_to_string(&path)
            .map_err(|error| format!("Could not read preview entry {}: {error}", path.display()))
    }

    fn schedule_project_index(&mut self) {
        self.project_index_deadline
            .schedule(Instant::now() + PROJECT_INDEX_DEBOUNCE);
    }

    fn rebuild_project_index(&mut self, context: &egui::Context) {
        self.project_index_deadline.clear();
        let main = self.preview_document_path();
        let root = self.project_root();
        let mut overrides = BTreeMap::new();
        if self.document.kind().is_typst() {
            let path = self.document.path().clone().unwrap_or_else(|| main.clone());
            overrides.insert(canonical_or_absolute(&path), self.document.source().clone());
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
            LatestJobPoll::Ready(index) if !self.project_index_deadline.is_pending() => {
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
        if self.document.take_edit().is_none() {
            return;
        }
        self.manual_format_revision = None;
        self.format_when_tinymist_ready = None;
        self.tooltip_request = None;
        self.notice = None;
        self.editor_hover = None;
        self.editor_completion = None;
        if self.document.kind().is_typst() {
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
        self.autosave_deadline = (self.snapshot_scene.is_none()
            && self.settings.auto_save
            && self.document.path().is_some()
            && self.document.source() != self.document.saved_source())
        .then(|| Instant::now() + Duration::from_millis(self.settings.auto_save_delay_ms.max(100)));
    }

    fn tick_autosave(&mut self, context: &egui::Context) {
        if self.snapshot_scene.is_some() {
            self.autosave_deadline = None;
            return;
        }
        if self.document_workflow.has_dialog() || self.document_workflow.modal().is_some() {
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
        let Some(path) = self.document.path().clone() else {
            return;
        };

        if self.save_to_with_intent(path, SaveIntent::Auto) {
            self.notice = Some(Notice {
                message: "Saved automatically".to_owned(),
                kind: NoticeKind::Success,
            });
        }
    }

    fn editor_revision(&self) -> DocumentKey {
        self.document.key()
    }

    fn prepare_editor_source_data(&mut self) {
        self.editor_data.prepare_source(&self.document.snapshot());
    }

    fn prepare_editor_data(&mut self) {
        self.prepare_editor_source_data();
        let document = self.editor_revision();
        let current_path = self.document.path().clone();
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
            revision: self.document.revision(),
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
                || result.revision != self.document.revision()
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
                    if self.preview.accepts_raster(key, self.document.revision()) =>
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
                    if self.preview.accepts_raster(key, self.document.revision()) =>
                {
                    self.preview.content.fail_raster(key, error.clone());
                    self.notice = Some(Notice {
                        message: format!(
                            "Preview is ready, but its raster preview is unavailable: {error}"
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
            if result.token != self.asset_token || !self.document.kind().preview_only() {
                continue;
            }
            match result.output {
                Ok(LoadedAsset::Image(page)) => {
                    let key = ArtifactKey::unversioned(self.document.revision());
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
                    let key = ArtifactKey::unversioned(self.document.revision());
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
                            Some(page.min(self.preview.content.pages().len().saturating_sub(1)));
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
                if let Err(error) = self.compiler.pause(self.document.revision()) {
                    self.compilation_paused = false;
                    self.notice = Some(Notice {
                        message: format!("Could not pause the Typst watcher: {error}"),
                        kind: NoticeKind::Error,
                    });
                    return;
                }
            }
            if let Some(generation) = self.tinymist_generation
                && let Err(error) = self
                    .tinymist
                    .set_preview_refresh(generation, PreviewRefresh::OnSave)
            {
                self.preview.tinymist_state = ServiceState::Degraded(error.to_string());
            }
            let (message, kind) = compilation_notice(true);
            self.notice = Some(Notice {
                message: message.to_owned(),
                kind,
            });
        } else {
            if let Some(generation) = self.tinymist_generation
                && let Err(error) = self
                    .tinymist
                    .set_preview_refresh(generation, PreviewRefresh::OnType)
            {
                self.preview.tinymist_state = ServiceState::Degraded(error.to_string());
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
            self.document.kind(),
            self.document.path().as_deref(),
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
        self.format_when_tinymist_ready = None;
        self.external_file_change_notice = None;
        self.next_external_file_check = Instant::now();
        self.asset_token.advance();
        self.asset_loader.cancel_before(self.asset_token);
        self.preview
            .clear_for_document(self.document.revision(), preserve_designated_preview);
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

    fn handle_shortcuts(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        let shortcut_viewport = focused_input_viewport(context);
        if self.handle_shortcut_capture(context, shortcut_viewport) {
            return;
        }
        let shortcuts = self.settings.effective_shortcuts();
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
                let key = self.document.key();
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
        let can_sync_preview = self.document.kind().is_typst() && self.interactive_preview_active();
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
                key != self.document.key()
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
                                key: self.document.key(),
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

        if self.view_mode.shows_preview() && !self.interactive_preview_active() {
            match context.input_mut_for(shortcut_viewport, |input| {
                consume_preview_zoom_shortcut(input, &shortcuts)
            }) {
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
            .egui(ShortcutAction::CloseWindow)
            .is_some_and(|shortcut| {
                context.input_mut_for(shortcut_viewport, |input| input.consume_shortcut(&shortcut))
            })
        {
            context.send_viewport_cmd_to(shortcut_viewport, egui::ViewportCommand::Close);
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
        frame: &eframe::Frame,
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
            AppCommand::Rename => {
                if let Some(path) = self.document.path().clone() {
                    self.begin_rename(path);
                }
            }
            AppCommand::Packages => self.open_package_manager(context),
            AppCommand::Git => self.git.open(context, &self.workspace_root),
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
                self.filesystem_phase = self.filesystem_phase.toggle();
                context.request_repaint();
            }
            AppCommand::Code => self.view_mode = ViewMode::Code,
            AppCommand::Split => self.view_mode = ViewMode::Split,
            AppCommand::Preview => self.view_mode = ViewMode::Preview,
        }
    }

    fn process_native_menu_commands(&mut self, context: &egui::Context, frame: &eframe::Frame) {
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
        if !visible {
            self.shortcut_editor_visible = false;
            self.shortcut_capture = None;
        }
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

    fn apply_snapshot_scene(&mut self) {
        let Some(scene) = self.snapshot_scene else {
            return;
        };
        if self.snapshot_document.is_none() {
            self.snapshot_document = Some(SceneDocument::capture(&self.document));
        }
        if matches!(
            scene,
            UiSnapshotScene::Main | UiSnapshotScene::ProblemsPanel | UiSnapshotScene::FindReplace
        ) && self.preview.content.pages().is_empty()
        {
            self.captures.defer_target("main");
        }
        if let Some(status) =
            settled_snapshot_preview_status(scene, !self.preview.content.pages().is_empty())
        {
            self.preview.status = status;
        }
        if matches!(
            scene,
            UiSnapshotScene::FontCompletion | UiSnapshotScene::SettingsFontPicker
        ) {
            if self.snapshot_font_file.is_none() {
                let definitions = egui::FontDefinitions::default();
                let key = &definitions.families[&egui::FontFamily::Monospace][0];
                let mut file = tempfile::NamedTempFile::new().expect("QA font fixture");
                std::io::Write::write_all(&mut file, definitions.font_data[key].font.as_ref())
                    .expect("write QA font fixture");
                self.snapshot_font_file = Some(file);
            }
            self.font_catalog =
                FontCatalog::single_font_fixture(self.snapshot_font_file.as_ref().unwrap().path());
        }
        let toolbar_anchor = Pos2::new(theme::SPACE.content, METRICS.chrome.toolbar_height);
        match scene {
            UiSnapshotScene::GitEditor | UiSnapshotScene::GitChunk => {
                const SOURCE: &str = "= Research notes\n\nThe revised model reaches 96% accuracy.\n\n== Method\nWe evaluate the model on three datasets.\n\nThe additional experiment confirms the result.\n\n== Results\nThe complete comparison follows below.\n\nThe remaining measurements agree.\n\n== Discussion\nThese observations support the revised approach.\n";
                let path = self
                    .document
                    .path()
                    .clone()
                    .unwrap_or_else(|| self.workspace_root.join("main.typ"));
                if self.document.source() != SOURCE {
                    self.document.replace_loaded(
                        SOURCE.into(),
                        path.clone(),
                        DocumentKind::Typst,
                        self.document.disk_fingerprint(),
                    );
                    self.prepare_editor_source_data();
                }
                self.notice = None;
                self.preview.status = PreviewStatus::Ready(Duration::ZERO);
                self.filesystem_phase = ExplorerPanelPhase::Open;
                self.view_mode = ViewMode::Code;
                self.git_editor = crate::git::editor::GitEditorState::snapshot_fixture(
                    &self.workspace_root,
                    &path,
                    SOURCE,
                    scene == UiSnapshotScene::GitChunk,
                );
                self.workspace = Some(WorkspaceTree::from_snapshot(WorkspaceSnapshot {
                    root: self.workspace_root.clone(),
                    nodes: [path, self.workspace_root.join("references.bib")]
                        .into_iter()
                        .map(|path| WorkspaceNode {
                            name: path.file_name().unwrap().to_owned(),
                            relative_path: path
                                .strip_prefix(&self.workspace_root)
                                .unwrap_or(&path)
                                .into(),
                            path,
                            kind: crate::workspace::WorkspaceNodeKind::File,
                            children: Vec::new(),
                        })
                        .collect(),
                }));
            }
            UiSnapshotScene::GitWindow => {
                if !self.git.visible {
                    self.git = crate::git::GitPanel::snapshot_fixture();
                }
            }
            UiSnapshotScene::AssetPreview => {}
            UiSnapshotScene::SettingsFontPicker => {
                self.settings_visible = true;
            }
            UiSnapshotScene::FontCompletion => {
                let source = "#set text(font: \"\")\n= Font completion";
                if self.document.source() != source {
                    self.document.replace_untitled(source);
                    self.prepare_editor_source_data();
                }
                self.view_mode = ViewMode::Code;
                self.pending_editor_selection = Some(17..17);
                self.request_editor_completion(
                    17,
                    Rect::from_min_size(Pos2::new(300.0, 180.0), Vec2::splat(1.0)),
                    true,
                );
            }
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
            UiSnapshotScene::StickyContext => {
                self.notice = None;
                self.view_mode = ViewMode::Code;
                self.problems_visible = false;
                self.find_visible = false;
                self.replace_visible = false;
                if prepare_sticky_context_snapshot_document(&mut self.document) {
                    self.prepare_editor_source_data();
                }
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
                if self.document_workflow.modal().is_none() {
                    self.document_workflow.set_modal(AppModal::Unsaved {
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
                if self.document_workflow.modal().is_none() {
                    self.document_workflow.set_modal(AppModal::Alert {
                        title: "error".to_owned(),
                        message:
                            "The document could not be saved. Check the destination and try again."
                                .to_owned(),
                        kind: NoticeKind::Error,
                    });
                }
            }
            UiSnapshotScene::OverwriteDialog => {
                if self.document_workflow.modal().is_none() {
                    self.document_workflow.set_modal(AppModal::Overwrite {
                        message: "This file changed on disk after it was opened. Overwrite it with the editor contents?"
                            .to_owned(),
                        path: self.document.path()
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
                    table: None,
                });
            }
            UiSnapshotScene::DocumentFontSelector => {
                self.document
                    .edit(CCursorRange::one(CCursor::new(0)), |source| {
                        *source =
                            "#set text(font: \"Libertinus Serif\")\n= Font selector".to_owned()
                    });
                self.prepare_editor_source_data();
                let font_char = self.document.source()
                    [..self.document.source().find("font").unwrap()]
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
                    .path()
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
                        detail: "Preview ready".to_owned(),
                        kind: NoticeKind::Success,
                    },
                    StatusLogEntry {
                        timestamp: "09:41:11Z".to_owned(),
                        detail: "Compiling document".to_owned(),
                        kind: NoticeKind::Info,
                    },
                    StatusLogEntry {
                        timestamp: "09:41:10Z".to_owned(),
                        detail: "Preview ready".to_owned(),
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
                        .path()
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
        self.git.visible = false;
        self.git_editor = crate::git::editor::GitEditorState::default();
        self.settings_visible = false;
        self.shortcut_editor_visible = false;
        self.packages_visible = false;
        self.typst_overrides_visible = false;
        self.workspace_chooser_visible = false;
        self.problems_visible = false;
        self.find_visible = false;
        self.replace_visible = false;
        self.view_mode = ViewMode::Split;
        self.search.clear();
        self.focus_find = false;
        self.pending_editor_selection = None;
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
        if let Some(fixture) = &self.snapshot_document {
            if fixture.restore(&mut self.document) {
                self.preview.content.clear();
            }
        } else {
            self.document.restore_saved_source();
        }
        self.theme_override = Some(step.theme.clone());
        self.snapshot_scene = Some(step.scene);
        if step.scene == UiSnapshotScene::StickyContext {
            self.document.reset_editor_history = true;
        }

        if matches!(
            step.scene,
            UiSnapshotScene::Main | UiSnapshotScene::ProblemsPanel | UiSnapshotScene::FindReplace
        ) && self.preview.content.pages().is_empty()
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

    fn handle_dropped_file(&mut self, context: &egui::Context) {
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
            self.is_dirty(),
            self.document_workflow.may_close(self.document.key()),
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
            .path()
            .as_ref()
            .is_some_and(|path| path.starts_with(&root))
        {
            if let Some(path) = self.document.path().clone() {
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
            .set_title("Open document in new window");
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.document_workflow.start_dialog(PendingDialog::new(
            DocumentDialogRequest {
                target: DocumentDialogTarget::OpenFileInNewWindow,
                key: self.document.key(),
            },
            dialog.pick_file(),
        ));
    }

    fn start_open_dialog(&mut self, frame: &eframe::Frame) {
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
            .document
            .path()
            .as_ref()
            .is_some_and(|current| same_path(current, &path));
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

        self.document
            .replace_loaded(source, path.clone(), kind, disk_fingerprint);
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
        } else if reloading_current_document
            && kind.is_typst()
            && self.tinymist_generation.is_some()
        {
            // Reloading an externally changed document only needs a fresh LSP
            // document. Keep the running Tinymist workspace and preview alive.
            self.reopen_tinymist_current_document(&path, kind);
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
        if !self.document.kind().is_editable() {
            return true;
        }
        if let Some(path) = self.document.path().clone() {
            self.save_to(path)
        } else {
            self.save_as(frame)
        }
    }

    fn save_as(&mut self, frame: &eframe::Frame) -> bool {
        if !self.document.kind().is_editable() {
            return false;
        }
        if self.document_workflow.has_dialog() {
            return false;
        }
        let typst = self.document.kind().is_typst();
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
        // Snapshot scenes freely replace and manipulate the in-memory source
        // to construct deterministic UI states. No capture path is allowed to
        // persist those synthetic edits to the fixture supplied on the CLI.
        if self.snapshot_scene.is_some() {
            self.autosave_deadline = None;
            return false;
        }
        let path_changed = self
            .document
            .path()
            .as_ref()
            .is_none_or(|current| !same_path(current, &path));
        if !path_changed {
            match intent {
                SaveIntent::Explicit if !self.confirm_disk_unchanged(&path) => return false,
                SaveIntent::ExplicitConfirmed => {}
                SaveIntent::Auto
                    if !disk_matches_fingerprint(&path, self.document.disk_fingerprint()) =>
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
        let saved_kind = if path_changed {
            crate::document::detect_document(&path, self.document.source().as_bytes())
                .ok()
                .filter(|kind| kind.is_editable())
                .unwrap_or(DocumentKind::Text)
        } else {
            self.document.kind()
        };
        let save = self
            .document
            .prepare_save(canonical_or_absolute(&path), saved_kind);
        let saved_fingerprint = fingerprint(save.source().as_bytes());
        match atomic_write(save.path(), save.source().as_bytes()) {
            Ok(durability) => {
                let path = path.canonicalize().unwrap_or(path);
                if !path.starts_with(&self.workspace_root)
                    && let Some(parent) = path.parent()
                {
                    self.workspace_root = canonical_or_absolute(&discover_project_root(parent));
                }
                let continuation = match self.document_workflow.complete_save(
                    &mut self.document,
                    save.committed(saved_fingerprint),
                    matches!(
                        durability,
                        crate::private_workspace::WriteDurability::Synchronized
                    ),
                ) {
                    Ok(continuation) => continuation,
                    Err(error) => {
                        self.show_file_error(error.to_owned());
                        return false;
                    }
                };
                self.external_file_change_notice = None;
                if path_changed {
                    self.preview.content.invalidate();
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
                if let Some(path) = self.document.path().clone() {
                    self.remember_open_document(&path);
                }
                self.schedule_project_index();
                if let crate::private_workspace::WriteDurability::Uncertain(error) = durability {
                    // Bytes are saved, so mark this snapshot clean, but keep the
                    // document open instead of silently completing a close.
                    self.document_workflow.cancel_continuation();
                    self.show_file_error(format!("Saved {}, but could not confirm disk durability: {error}. The document remains open.", path.display()));
                    return false;
                }
                if let Some(mut action) = continuation {
                    action.key = self.document.key();
                    action.allow_discard = false;
                    self.document_workflow.queue_action(action);
                }
                true
            }
            Err(error) => {
                match intent {
                    SaveIntent::Explicit | SaveIntent::ExplicitConfirmed => {
                        self.document_workflow.cancel_continuation();
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
        let Some(expected) = self.document.disk_fingerprint() else {
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
                self.document_workflow.cancel_continuation();
                self.show_file_error(format!(
                    "Could not check {} before saving: {error}",
                    path.display()
                ));
                return false;
            }
        };

        self.document_workflow.set_modal(AppModal::Overwrite {
            message: description,
            path: path.to_path_buf(),
            key: self.document.key(),
            expected_disk_fingerprint: self.document.disk_fingerprint(),
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
        if !self.typst_preview_available() && self.document.kind() != DocumentKind::Pdf {
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
            self.document.path().clone()
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
                document_epoch: self.document.epoch(),
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
            if self.document.epoch() == request.document_epoch {
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
        match target {
            DocumentDialogTarget::OpenFile => {
                if self.document.epoch() != key.epoch {
                    self.notice = Some(Notice {
                        message: "Open canceled because another document is now active".to_owned(),
                        kind: NoticeKind::Info,
                    });
                } else if self.document.revision() != key.revision {
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
                if self.document.epoch() != key.epoch {
                    self.notice = Some(Notice {
                        message: "Open Folder canceled because another document is now active"
                            .to_owned(),
                        kind: NoticeKind::Info,
                    });
                } else if self.document.revision() != key.revision {
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
                if self.document.epoch() != key.epoch {
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
                let saved = self.save_to(path);
                if let Some(key) =
                    save_as_format_handoff(saved, self.document.kind(), self.document.key())
                {
                    // Save As changes the document URI and restarts Tinymist.
                    // Remember this exact saved revision and request formatting
                    // only after the replacement LSP session has opened it.
                    self.format_when_tinymist_ready = Some(key);
                    self.notice = Some(Notice {
                        message: "Saved; formatting when Tinymist is ready…".to_owned(),
                        kind: NoticeKind::Info,
                    });
                }
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
        let requires_new_artifact = pdf_output_requires_new_artifact(
            typst_output,
            self.compilation_paused,
            self.compile_deadline.is_some(),
            self.preview.status,
        );
        let reusable = pdf_artifact_reusable_for_output(
            self.preview.content.artifact_key(),
            self.preview.content.pdf().is_some(),
            self.document.revision(),
            requires_new_artifact,
        );
        self.document_workflow.pending_export = Some(PendingExport {
            path,
            document_epoch: self.document.epoch(),
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
            self.schedule_compile_now();
        }
    }

    fn complete_pending_export(&mut self) {
        let Some(pdf) = self.preview.content.pdf() else {
            return;
        };
        let Some(pending) = take_ready_export(
            &mut self.document_workflow.pending_export,
            self.document.epoch(),
            self.document.revision(),
            self.preview.content.artifact_key(),
        ) else {
            return;
        };
        match atomic_write(&pending.path, pdf) {
            Err(error) => self.show_file_error(error),
            Ok(crate::private_workspace::WriteDurability::Uncertain(error)) => {
                self.show_file_error(format!(
                    "Wrote {}, but could not confirm disk durability: {error}",
                    pending.path.display()
                ));
            }
            Ok(crate::private_workspace::WriteDurability::Synchronized) => {
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
            let _ = self.compiler.pause(self.document.revision());
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
        self.execute_pending_document_action_inner(context, frame);
        self.document_workflow.finish_dispatch();
    }

    fn execute_pending_document_action_inner(
        &mut self,
        context: &egui::Context,
        frame: &eframe::Frame,
    ) {
        let Some(mut pending) = self.document_workflow.take_action() else {
            return;
        };
        if pending.key.epoch != self.document.epoch() {
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
                if self.save_document(frame)
                    && let Some(mut action) = self.document_workflow.take_continuation()
                {
                    action.key = self.document.key();
                    action.allow_discard = false;
                    self.document_workflow.queue_action(action);
                }
                return;
            }
            DeferredDocumentAction::ForceSave {
                path,
                key,
                expected_disk_fingerprint,
                observed_disk_fingerprint,
            } => {
                let same_document = key.epoch == self.document.epoch()
                    && self
                        .document
                        .path()
                        .as_ref()
                        .is_some_and(|current| same_path(current, &path))
                    && self.document.disk_fingerprint() == expected_disk_fingerprint;
                if !same_document {
                    self.document_workflow.cancel_continuation();
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
                        self.document_workflow.cancel_continuation();
                        self.show_file_error(format!(
                            "Could not recheck {} before saving: {error}",
                            path.display()
                        ));
                        return;
                    }
                };
                let continue_after_save = self.document_workflow.has_continuation();
                let saved = if key.revision != self.document.revision()
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
            if pending.key.revision != self.document.revision() {
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
                self.document_workflow.allow_close_for(self.document.key());
                if !self.process_close_pending {
                    context.send_viewport_cmd(egui::ViewportCommand::Close);
                }
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
                revision_as_i32(self.document.revision()),
                self.document.source(),
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
                revision_as_i32(self.document.revision()),
                self.document.source().clone(),
            ) {
                self.preview.tinymist_state = ServiceState::Degraded(error.to_string());
            }
            return;
        }
        if self.preview.connection.is_ready() {
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
        self.git_editor.refresh();
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
        if !self.filesystem_phase.panel_visible() {
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
        let Some(path) = self.document.path().clone() else {
            return;
        };
        if !self.document.kind().is_editable() {
            return;
        }
        let observed = match fs::read(&path) {
            Ok(contents) => ExternalFileObservation::Present(fingerprint(&contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ExternalFileObservation::Missing
            }
            Err(_) => return,
        };
        if matches!(observed, ExternalFileObservation::Present(fingerprint) if Some(fingerprint) == self.document.disk_fingerprint())
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
            .or_else(|| self.document.path().clone())
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
        self.editor_completion = None;
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
                self.webview_url = None;
                self.webview_navigation = None;
            }
        }
        if !self.document.kind().is_typst() && self.designated_preview_path().is_none() {
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
        config.preview.refresh = tinymist_preview_refresh(self.compilation_paused);
        match self.tinymist.start_workspace(config) {
            Ok(generation) => {
                let version = revision_as_i32(self.document.revision());
                let current_document = if self.document.kind().is_typst() {
                    if let Some(path) = self.document.path().as_deref() {
                        TextDocument::from_path(path, version, self.document.source().clone())
                            .map_err(|error| error.to_string())
                    } else {
                        let source_dir = self
                            .current_directory()
                            .unwrap_or_else(|| self.project_root());
                        match UnsavedTextDocument::create(
                            self.project_root(),
                            source_dir,
                            self.document_name(),
                            self.document.source(),
                        ) {
                            Ok(document) => {
                                let text_document =
                                    document.text_document(version, self.document.source().clone());
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
                    if !self.document.kind().is_typst() || self.current_is_preview_document() {
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
                self.preview.connection.start(generation);
                self.tinymist_current_open = current_document.uri == preview_document.uri;
                // The first document opened determines startDefaultPreview.
                // Imported/current subfiles are opened after Initialized.
                if let Err(error) = self.tinymist.did_open(generation, preview_document) {
                    self.preview.tinymist_state = ServiceState::Failed(error.to_string());
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

    fn sync_tinymist_change(&self) -> Result<(), String> {
        if !self.document.kind().is_typst() {
            return Ok(());
        }
        if let Some(document) = &self.tinymist_unsaved_document {
            document
                .update_backing_source(self.document.source())
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
                revision_as_i32(self.document.revision()),
                self.document.source().clone(),
            )
            .map_err(|error| error.to_string())
    }

    fn receive_tinymist_events(&mut self, context: &egui::Context) {
        while let Some(event) = self.tinymist.try_recv() {
            match event {
                TinymistEvent::Starting { .. } => {
                    self.editor_completion = None;
                    self.preview.tinymist_state =
                        ServiceState::Starting("Launching Tinymist LSP".to_owned());
                }
                TinymistEvent::Initialized { generation } => {
                    if !self.preview.connection.initialized(generation) {
                        continue;
                    }
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
                            revision_as_i32(self.document.revision()),
                            self.document.source().clone(),
                        );
                        match self.tinymist.did_open(generation, document) {
                            Ok(()) => self.tinymist_current_open = true,
                            Err(error) => {
                                self.preview.tinymist_state =
                                    ServiceState::Degraded(error.to_string())
                            }
                        }
                    }
                    if take_ready_format_handoff(
                        &mut self.format_when_tinymist_ready,
                        self.document.key(),
                        self.tinymist_generation == Some(generation) && self.tinymist_current_open,
                    ) {
                        self.request_format_after_manual_save();
                    }
                }
                TinymistEvent::PreviewReady { generation, url } => {
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
                            self.tinymist_generation,
                            self.tinymist_uri.as_deref(),
                            revision_as_i32(self.document.revision()),
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
                        self.preview.connection.clear_endpoint();
                        self.preview.webview_state = ServiceState::Failed(message.clone());
                    }
                    let detail = format!("{stage}: {message}");
                    self.preview.tinymist_state = if fatal {
                        self.format_when_tinymist_ready = None;
                        self.preview.connection.stop();
                        self.preview.connection.clear_endpoint();
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
                    self.format_when_tinymist_ready = None;
                    self.editor_completion = None;
                    self.preview.connection.stop();
                    self.preview.connection.clear_endpoint();
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
        while let Ok(target) = self.web_link_receiver.try_recv() {
            self.handle_web_link(&target);
        }
    }

    fn handle_web_link(&mut self, target: &str) {
        self.follow_preview_link(target);
    }

    fn follow_preview_link(&mut self, target: &str) {
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
            self.open_external_link(url.as_str());
        } else {
            self.notice = Some(Notice {
                message: format!("Unsupported link scheme: {}", url.scheme()),
                kind: NoticeKind::Error,
            });
        }
    }

    fn open_external_link(&mut self, target: &str) {
        match open_in_system_browser(target) {
            Ok(()) => self.notice = Some(external_link_opened_notice(target)),
            Err(error) => {
                self.notice = Some(Notice {
                    message: error,
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
        let same_document = self
            .document
            .path()
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
        if self.document.kind() == DocumentKind::Pdf {
            if self.preview.content.pages().is_empty() {
                self.pending_asset_page = page;
            } else if let Some(page) = page {
                self.preview.requested_page =
                    Some(page.min(self.preview.content.pages().len().saturating_sub(1)));
            }
        } else if self.document.kind().is_editable()
            && let Some((line, column)) = source_position
        {
            let char_index = char_index_at_line_column(self.document.source(), line, column);
            self.pending_editor_selection = Some(char_index..char_index);
            self.editor_attention = Some(EditorAttention {
                char_index,
                started: Instant::now(),
            });
            if self.document.kind().is_typst() {
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
            self.document.path().is_none() && path == self.tinymist_document_path();
        let same_document = virtual_untitled
            || self
                .document
                .path()
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
            let range = range_to_scalar_range(self.document.source(), selection).into_range();
            self.editor_attention = Some(EditorAttention {
                char_index: range.start,
                started: Instant::now(),
            });
            self.pending_editor_selection = Some(range);
        }
        self.view_mode = ViewMode::Split;
    }

    fn jump_source_to_preview(&mut self, char_index: usize) {
        if !self.document.kind().is_typst() || !self.interactive_preview_active() {
            return;
        }
        let Some(generation) = self.tinymist_generation else {
            return;
        };
        let (line, character) =
            scalar_position_at(self.document.source(), ScalarOffset::new(char_index));
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
            self.document.kind(),
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
            && version.is_some_and(|version| version != revision_as_i32(self.document.revision()))
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
            return match self.document.kind() {
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
            .content
            .raster_key()
            .unwrap_or_else(|| ArtifactKey::unversioned(self.document.revision()));
        let dark = self.preview.dark
            && (self.typst_preview_available() || self.document.kind() != DocumentKind::Image);
        for (index, page) in self.preview.content.pages_mut().iter_mut().enumerate() {
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
        let shortcuts = self.settings.effective_shortcuts();
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
            let view_mode_enabled = view_mode_controls_enabled(self.document.kind());
            if compact {
                theme::apply_dense_toolbar_spacing(ui);
            }

            theme::show_logo(ui);
            let document_name = self.document_name();
            let title = format!("{document_name}{}", if self.is_dirty() { "*" } else { "" });
            // Reserve the dirty marker's slot even while the document is
            // clean, so saving never shifts the menu and view controls.
            let natural_title_width = (document_name.chars().count() + 1) as f32
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
                    self.document.path().as_ref().map_or_else(
                        || "Unsaved document".to_owned(),
                        |path| path.display().to_string()
                    )
                ),
            );
            if title_response.double_clicked() && !self.document_flow_busy() {
                if let Some(path) = self.document.path().clone() {
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
            if native_hover_text(
                ui.button("Find"),
                shortcut_tooltip("Find", &shortcuts, ShortcutAction::Find),
            )
            .clicked()
            {
                self.toggle_find();
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

            // Consuming the remaining width with a right-to-left layout keeps
            // the view controls pinned to the opposite edge while the title is
            // the only item that contracts at narrow window sizes.
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
                    ui.selectable_label(self.filesystem_phase.panel_visible(), explorer_label),
                    "Toggle the file explorer",
                )
                .clicked()
                {
                    self.filesystem_phase = self.filesystem_phase.toggle();
                    ui.ctx().request_repaint();
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
        if !self.document.kind().is_editable() {
            return;
        }
        let current = self.editor_snapshot(context);
        let previous_key = self.document.key();
        let next = self.document.history_step(redo, current.cursor);
        let Some(next) = next else {
            return;
        };
        let changed = previous_key != self.document.key();
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
            self.document.source().chars().count(),
        )));
        state.store(context, source_editor_id(context));
    }

    fn selected_editor_chars(&self, context: &egui::Context) -> Option<Range<usize>> {
        let state = egui::text_edit::TextEditState::load(context, source_editor_id(context))?;
        let range = state.cursor.char_range()?.as_sorted_char_range();
        let len = self.document.source().chars().count();
        let range = range.start.0.min(len)..range.end.0.min(len);
        (range.start != range.end).then_some(range)
    }

    fn copy_editor_selection(&mut self, context: &egui::Context) {
        let Some(range) = self.selected_editor_chars(context) else {
            return;
        };
        self.prepare_editor_source_data();
        let bytes = self.editor_data.char_range_to_byte(range);
        context.copy_text(self.document.source()[bytes].to_owned());
    }

    fn cut_editor_selection(&mut self, context: &egui::Context) {
        if !self.document.kind().is_editable() {
            return;
        }
        let Some(range) = self.selected_editor_chars(context) else {
            return;
        };
        let cursor = self.editor_snapshot(context).cursor;
        let bytes = self.editor_data.char_range_to_byte(range.clone());
        context.copy_text(self.document.source()[bytes.clone()].to_owned());
        self.document
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
        if self.document.kind().is_editable() {
            if self.document.kind().is_typst() && !self.view_mode.shows_code() {
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
                CCursor::new(self.document.source().chars().count()),
            )));
            state.store(context, editor_id);
            context.memory_mut(|memory| memory.request_focus(editor_id));
        }
    }

    fn toggle_comments(&mut self, context: &egui::Context) {
        if !self.document.kind().is_editable() {
            return;
        }
        let editor_id = source_editor_id(context);
        let state = egui::text_edit::TextEditState::load(context, editor_id);
        let cursor = state
            .as_ref()
            .and_then(|state| state.cursor.char_range())
            .map_or(0, |range| range.primary.index.0)
            .min(self.document.source().chars().count());
        let selection = state
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.as_sorted_char_range())
            .map(|range| {
                range.start.0.min(self.document.source().chars().count())
                    ..range.end.0.min(self.document.source().chars().count())
            })
            .filter(|range| range.start != range.end);
        let (source, mapped_range) = toggle_line_comments(
            self.document.source(),
            selection.clone().unwrap_or(cursor..cursor),
            "// ",
        );
        if source == *self.document.source() {
            return;
        }
        let cursor = self.editor_snapshot(context).cursor;
        self.document.edit(cursor, |buffer| *buffer = source);
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
        if !self.document.kind().is_typst() {
            self.manual_format_revision = None;
            return;
        }
        if !self.preview.connection.is_ready() {
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
            revision_as_i32(self.document.revision()),
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
        if self.document.kind().is_typst() {
            self.manual_format_revision = Some(self.document.revision());
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
            || version != revision_as_i32(self.document.revision())
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
            self.document.source(),
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
        if formatted == *self.document.source() {
            self.manual_format_revision = None;
            self.notice = Some(Notice {
                message: "Document is already formatted".to_owned(),
                kind: NoticeKind::Success,
            });
            return;
        }
        self.pending_editor_selection = None;
        let save_after_format = self.manual_format_revision == Some(self.document.revision());
        self.document.edit(cursor, |source| *source = formatted);
        self.store_editor_cursor(context, mapped_cursor);
        self.document.reset_editor_history = false;
        self.search.clear();
        self.manual_format_revision = None;
        self.mark_edited();
        let saved_after_format = if save_after_format {
            if let Some(path) = self.document.path().clone() {
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

    fn begin_table_editor(&mut self, table: EditableTable) {
        let Some(original_call) =
            char_range_slice(self.document.source(), table.source_range.clone())
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
            document_key: self.document.key(),
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
            .document
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
            renames_current_document && self.document.kind().preview_only();
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
            // bytes. Using `self.document.source()` here would pass an empty buffer for
            // PDFs/images and could incorrectly turn them into editable text.
            self.load_path(new_path.clone());
        } else if renames_current_document {
            let kind =
                crate::document::detect_document(&new_path, self.document.source().as_bytes())
                    .ok()
                    .filter(|kind| kind.is_editable())
                    .unwrap_or(DocumentKind::Text);
            self.document.rename(new_path.clone(), kind);
            self.remember_open_document(&new_path);
            if preserve_designated_preview {
                self.reopen_tinymist_current_document(&new_path, self.document.kind());
                self.refresh_workspace();
            } else {
                self.reset_document_services();
            }
            if self.document.kind().is_typst() {
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

    fn execute_pending_app_popup_action(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        let Some(action) = self.pending_app_popup_action.take() else {
            return;
        };
        match action {
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
                        .document
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
                        self.autosave_deadline = None;
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
        if target.value_range.end > self.document.source().len()
            || !self
                .document
                .source()
                .is_char_boundary(target.value_range.start)
            || !self
                .document
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
        let selection_end = self.document.source()[..target.value_range.start]
            .chars()
            .count()
            + replacement.chars().count();
        self.document.edit(snapshot.cursor, |source| {
            source.replace_range(target.value_range, &replacement)
        });
        self.pending_editor_selection = Some(selection_end..selection_end);
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
        if self.document.kind() == DocumentKind::Text && !self.typst_preview_available() {
            return ServiceState::Disabled("Text files do not need a preview renderer".to_owned());
        }
        if self.document.kind() == DocumentKind::Image
            && !self.typst_preview_available()
            && !self.preview.content.pages().is_empty()
        {
            return ServiceState::Ready("The selected image decoded successfully".to_owned());
        }
        if self.raster_content_freshness() == Some(RasterContentFreshness::Current) {
            return ServiceState::Ready(format!(
                "Poppler rendered {} page(s) at {} DPI",
                self.preview.content.pages().len(),
                crate::compiler::PREVIEW_DPI
            ));
        }
        if self.raster_content_freshness() == Some(RasterContentFreshness::Stale) {
            return ServiceState::Degraded(format!(
                "Showing {} page(s) from the last successful build",
                self.preview.content.pages().len()
            ));
        }
        if let Some(error) = self.preview.content.error() {
            return ServiceState::Degraded(error.to_owned());
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
        self.preview.raster_freshness(self.document.revision())
    }

    fn show_workspace(&mut self, ui: &mut egui::Ui) {
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
        theme::panel_header(ui, "workspace-search-header", |ui| {
            let show_clear = !self.explorer_query.is_empty();
            let clear_width = if show_clear {
                METRICS.icon.button_size.x + ui.spacing().item_spacing.x
            } else {
                0.0
            };
            ui.add_sized(
                [
                    (ui.available_width() - clear_width).max(1.0),
                    METRICS.explorer.header_row_height,
                ],
                egui::TextEdit::singleline(&mut self.explorer_query)
                    .id_salt("explorer-search")
                    .hint_text("Search Explorer"),
            );
            if show_clear && icon_button(ui, UiIcon::Close, "Clear Explorer search").clicked() {
                self.explorer_query.clear();
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
            self.document.path().as_ref().and_then(|path| {
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
        let explorer_query = normalize_explorer_query(&self.explorer_query);
        let filter_active = !explorer_query.is_empty();
        let section_defaults = if filter_active {
            explorer_section_query_matches(snapshot, project_index, &explorer_query)
        } else {
            std::array::from_fn(|index| EXPLORER_SECTION_SPECS[index].1)
        };
        let open_sections = explorer_section_open_states(ui, filter_active, section_defaults);
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
        let mut section_resize = None;
        let resize_delta = explorer_section_resizable(
            ui,
            ExplorerSectionRenderSpec {
                id_salt: "workspace-files",
                title: "Files",
                default_open: section_defaults[0],
                body_height: section_body_heights[0],
                show_resize_handle: next_open_explorer_section(open_sections, 0).is_some(),
                filtered: filter_active,
            },
            |ui| {
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
            },
        );
        if resize_delta.abs() > f32::EPSILON {
            section_resize = Some((0, resize_delta));
        }

        let ExplorerProjectSectionsOutcome {
            resize_request,
            open_package_manager,
            target: index_target,
        } = show_project_index_sections(
            ui,
            ExplorerProjectSectionsSpec {
                root: project_root,
                index: project_index,
                query: &explorer_query,
                filtered: filter_active,
                section_defaults,
                open_sections,
                body_heights: section_body_heights,
            },
        );
        if resize_request.is_some() {
            section_resize = resize_request;
        }

        if open_package_manager {
            self.open_package_manager(ui.ctx());
        }

        if let Some((section, delta)) = section_resize
            && section_layout.resize_after(open_sections, section_body_budget, section, delta)
        {
            ui.ctx()
                .data_mut(|data| data.insert_temp(section_layout_id, section_layout));
            ui.ctx().request_repaint();
        }

        if let Some(path) = open_path
            && self
                .document
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
            if self
                .document
                .path()
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
        let version = revision_as_i32(self.document.revision());
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
        let opacity = if self.tooltip_request.is_some() {
            Some(1.0)
        } else {
            hover_opacity(
                &response,
                native_hover_tooltip_id(ui.ctx()).with("semantic-hover-timing"),
            )
        };

        let should_request = self.preview.connection.is_ready()
            && self.tinymist_current_open
            && self
                .editor_hover
                .as_ref()
                .is_some_and(|hover| !hover.requested);
        if should_request {
            let position =
                lsp_position_at_scalar(self.document.source(), ScalarOffset::new(range.start));
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
        if self.document.kind().is_typst()
            && let Some(all_items) =
                crate::completion::font_items(self.document.source(), cursor, &self.font_catalog)
        {
            let items =
                crate::completion::filtered_for_source(&all_items, self.document.source(), cursor);
            self.editor_completion = Some(EditorCompletionState {
                generation: self.tinymist_generation.unwrap_or(Generation(0)),
                uri: self.tinymist_uri.clone().unwrap_or_default(),
                version: revision_as_i32(self.document.revision()),
                request_token: 0,
                cursor,
                anchor,
                explicit,
                is_incomplete: false,
                selected: 0,
                items,
                all_items,
                source: self.document.source().clone(),
                local: true,
            });
            return;
        }
        if !explicit
            && self
                .document
                .source()
                .chars()
                .nth(cursor.saturating_sub(1))
                .is_some_and(|c| c.is_whitespace() || c == '"')
        {
            self.editor_completion = None;
            return;
        }
        let ready = tinymist_language_features_ready(
            self.document.kind(),
            self.preview.connection.is_ready(),
            self.tinymist_current_open,
        );
        let (Some(generation), Some(uri)) = (self.tinymist_generation, self.tinymist_uri.clone())
        else {
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

        let cursor = cursor.min(self.document.source().chars().count());
        let version = revision_as_i32(self.document.revision());
        let request_token = self.next_editor_completion_token;
        self.next_editor_completion_token =
            self.next_editor_completion_token.wrapping_add(1).max(1);
        let position = lsp_position_at_scalar(self.document.source(), ScalarOffset::new(cursor));
        match self.tinymist.complete_document(
            generation,
            uri.clone(),
            version,
            position,
            request_token,
        ) {
            Ok(()) => {
                self.editor_completion = Some(EditorCompletionState {
                    generation,
                    uri,
                    version,
                    request_token,
                    cursor,
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
                    source: self.document.source().clone(),
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
        let Some(completion) = &mut self.editor_completion else {
            return;
        };
        let current = completion_response_matches(
            completion,
            generation,
            &uri,
            version,
            request_token,
            self.tinymist_generation,
            self.tinymist_uri.as_deref(),
            revision_as_i32(self.document.revision()),
        );
        if !current {
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
            self.document.source(),
            completion.cursor,
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
            || (self.tinymist_generation == Some(completion.generation)
                && self.tinymist_uri.as_deref() == Some(completion.uri.as_str())))
            && completion.version == revision_as_i32(self.document.revision());
        if !current {
            self.editor_completion = None;
            return;
        }
        let Some(item) = completion.items.get(index).cloned() else {
            return;
        };
        let request_cursor = completion.cursor;
        let application =
            match prepare_completion_application(self.document.source(), request_cursor, &item) {
                Ok(application) => application,
                Err(error) => {
                    self.editor_completion = None;
                    self.notice = Some(Notice {
                        message: format!("Could not apply completion: {error}"),
                        kind: NoticeKind::Error,
                    });
                    return;
                }
            };

        let snapshot = self.editor_snapshot(context);
        self.document
            .edit(snapshot.cursor, |source| *source = application.source);
        self.pending_editor_selection = Some(application.cursor..application.cursor);
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

    fn diagnostic_targets_current_document(&self, diagnostic: &Diagnostic) -> bool {
        match &diagnostic.source {
            DiagnosticSource::Main => self.current_is_preview_document(),
            DiagnosticSource::File(path) => {
                self.document
                    .path()
                    .as_ref()
                    .is_some_and(|current| same_path(current, path))
                    || (self.document.path().is_none()
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
                !self.preview.content.pages().is_empty(),
                self.compile_deadline.is_some(),
                self.preview.status,
                self.preview.content.artifact_key().is_some(),
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
                    self.preview.content.pages().len()
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
        if self.preview.content.pages().is_empty()
            || snapshot_scene_hides_preview_pages(self.snapshot_scene)
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
            .content
            .pages()
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
            self.preview.content.pages().iter().map(|page| page.size),
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
                for (page, geometry) in self.preview.content.pages().iter().zip(&geometries) {
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
        if self.document.kind().is_typst() {
            return None;
        }
        self.document
            .path()
            .as_ref()
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
            .or_else(|| {
                Some(
                    match self.document.kind() {
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
        if !self.document.kind().is_editable() {
            return None;
        }
        let char_index = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .and_then(|state| state.cursor.char_range())
            .map_or(0, |range| range.primary.index.0)
            .min(self.document.source().chars().count());
        Some(line_column_at_char(self.document.source(), char_index))
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
            if self.document.kind().is_editable() {
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

            let (icon, color, timing) =
                if !self.may_run_compilation() && self.typst_preview_available() {
                    (UiIcon::Waiting, neutral_color(ui.ctx()), None)
                } else {
                    match self.preview.status {
                        PreviewStatus::Waiting => (UiIcon::Waiting, neutral_color(ui.ctx()), None),
                        PreviewStatus::Compiling => (UiIcon::Refresh, info_color(ui.ctx()), None),
                        PreviewStatus::Ready(elapsed) => (
                            UiIcon::Check,
                            success_color(ui.ctx()),
                            preview_timing_label(self.document.kind(), elapsed),
                        ),
                        PreviewStatus::Error => (UiIcon::Warning, error_color(ui.ctx()), None),
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
            if let Some(notice) = &self.notice {
                ui.separator();
                let color = match notice.kind {
                    NoticeKind::Info => info_color(ui.ctx()),
                    NoticeKind::Success => success_color(ui.ctx()),
                    NoticeKind::Error => error_color(ui.ctx()),
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
            return match (self.document.kind(), self.preview.status) {
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
            PreviewStatus::Ready(_) => "Preview ready".to_owned(),
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
                && self.document.path().is_some()
                && self.is_dirty())
            .then(|| {
                Instant::now() + Duration::from_millis(self.settings.auto_save_delay_ms.max(100))
            });
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
        let tooltip_retained = native_tooltip_handoff_active(&context, true);
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
        self.receive_compile_results(&context);
        self.receive_asset_results(&context);
        self.receive_asset_thumbnail_results(&context);
        self.poll_export_dialog(&context);
        self.poll_tool_picker(&context);
        self.poll_document_dialog(&context);
        self.receive_tinymist_events(&context);
        self.receive_web_links();
        self.handle_shortcuts(&context, frame);
        // Menu-driven TextEdit commands may inject semantic events. Process
        // them after shortcut normalization so they reach the focused widget
        // unchanged during the UI pass below.
        self.process_native_menu_commands(&context, frame);
        self.execute_pending_app_popup_action(&context, frame);
        let drop_id = viewport_scoped_id(&context, "file-drop-target");
        context.data_mut(|data| data.remove::<FileDropTarget>(drop_id));
        let force_hover_id = viewport_scoped_id(&context, "force-pointer-tooltip");
        let force_hover = matches!(self.tooltip_request, Some(TooltipRequest::Pointer(pos)) if context.pointer_latest_pos() == Some(pos));
        context.data_mut(|data| data.insert_temp(force_hover_id, force_hover));
        self.handle_close_request(&context);
        self.update_title(&context);
        self.tick_workspace(&context);
        self.tick_project_index(&context);
        self.apply_snapshot_scene();
        if self.snapshot_scene.is_none() {
            self.git_editor.tick(
                &context,
                &self.workspace_root,
                self.document
                    .path()
                    .as_deref()
                    .filter(|_| self.document.kind().is_editable()),
                self.document.key(),
                self.document.source(),
            );
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
        if self.filesystem_phase.panel_visible() {
            let show_contents = self.filesystem_phase.contents_visible();
            egui::Panel::left("filesystem")
                .frame(theme::content_panel_frame(ui.style()))
                .resizable(true)
                .default_size(METRICS.chrome.explorer_default_width)
                .min_size(METRICS.chrome.explorer_min_width)
                .show(ui, |ui| {
                    if show_contents {
                        self.show_workspace(ui);
                    }
                });
            let next_phase = self.filesystem_phase.finish_frame();
            if next_phase != self.filesystem_phase {
                self.filesystem_phase = next_phase;
                context.request_repaint();
            }
        }
        let designated_preview = self.designated_preview_path().is_some();
        if self.document.kind().preview_only() {
            // This presentation override intentionally does not mutate
            // `view_mode`: returning to a source file restores the user's Code,
            // Split, or Preview preference.
            egui::CentralPanel::default()
                .frame(theme::content_panel_frame(ui.style()))
                .show(ui, |ui| self.show_preview(ui, frame));
        } else if self.document.kind() == DocumentKind::Text && !designated_preview {
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
        self.show_typst_overrides_window(&context);
        self.show_workspace_chooser(&context);
        self.show_package_manager_window(&context);
        self.show_git_window(&context);
        self.show_git_chunk_window(&context);
        // Every transaction is observed, including edits introduced by new
        // commands which do not explicitly request immediate service updates.
        self.mark_edited();
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

fn completion_requested_after_events(events: &[egui::Event]) -> bool {
    events.iter().any(|event| match event {
        egui::Event::Text(text) => text.chars().last().is_some_and(|character| {
            character.is_alphanumeric()
                || matches!(
                    character,
                    '_' | '-' | '.' | '#' | '@' | ':' | '/' | ' ' | '"'
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

fn completion_popup_position(anchor: Rect, desired_size: Vec2, viewport: Rect) -> Pos2 {
    let edge = 4.0;
    let gap = theme::SPACE.tight;
    let min_x = viewport.left() + edge;
    let max_x = (viewport.right() - edge - desired_size.x).max(min_x);
    let x = anchor.left().clamp(min_x, max_x);
    let below = anchor.bottom() + gap;
    let above = anchor.top() - gap - desired_size.y;
    let preferred_y =
        if below + desired_size.y <= viewport.bottom() - edge || above < viewport.top() + edge {
            below
        } else {
            above
        };
    let min_y = viewport.top() + edge;
    let max_y = (viewport.bottom() - edge - desired_size.y).max(min_y);
    Pos2::new(x, preferred_y.clamp(min_y, max_y))
}

fn completion_prefix_range(source: &str, cursor: usize) -> Range<usize> {
    let cursor = cursor.min(source.chars().count());
    let prefix = source.chars().take(cursor).collect::<Vec<_>>();
    let start = prefix
        .iter()
        .rev()
        .take_while(|&&character| character.is_alphanumeric() || matches!(character, '_' | '-'))
        .count();
    cursor.saturating_sub(start)..cursor
}

fn completion_ranges_conflict(left: &Range<usize>, right: &Range<usize>) -> bool {
    if left.is_empty() && right.is_empty() {
        return left.start == right.start;
    }
    if left.is_empty() {
        return right.start <= left.start && left.start <= right.end;
    }
    if right.is_empty() {
        return left.start <= right.start && right.start <= left.end;
    }
    left.start < right.end && right.start < left.end
}

fn prepare_completion_application(
    source: &str,
    request_cursor: usize,
    item: &CompletionItem,
) -> Result<CompletionApplication, String> {
    let source_len = source.chars().count();
    if request_cursor > source_len {
        return Err("the completion cursor is outside the document".to_owned());
    }

    let mut main_edit = item.text_edit.clone().unwrap_or_else(|| {
        let range = completion_prefix_range(source, request_cursor);
        LspTextEdit {
            range: LspRange {
                start: lsp_position_at_scalar(source, ScalarOffset::new(range.start)),
                end: lsp_position_at_scalar(source, ScalarOffset::new(range.end)),
            },
            new_text: item.insert_text.clone(),
        }
    });
    let expansion = if item.insert_text_is_snippet {
        expand_lsp_snippet(&main_edit.new_text)?
    } else {
        SnippetExpansion {
            cursor: main_edit.new_text.chars().count(),
            text: main_edit.new_text.clone(),
        }
    };
    main_edit.new_text = expansion.text.clone();
    let main_range = range_to_scalar_range(source, &main_edit.range).into_range();

    for additional in &item.additional_text_edits {
        let additional_range = range_to_scalar_range(source, &additional.range).into_range();
        if completion_ranges_conflict(&main_range, &additional_range) {
            return Err("the completion's main and additional edits overlap".to_owned());
        }
    }

    let mut edits = Vec::with_capacity(1 + item.additional_text_edits.len());
    edits.push(main_edit);
    edits.extend(item.additional_text_edits.iter().cloned());
    let applied = apply_text_edits(
        source,
        &edits,
        ([main_range.end, main_range.end]).map(ScalarOffset::new),
    )?;
    let inserted_len = expansion.text.chars().count();
    let cursor = applied.mapped_offsets[0]
        .get()
        .saturating_sub(inserted_len)
        .saturating_add(expansion.cursor.min(inserted_len));
    Ok(CompletionApplication {
        source: applied.text,
        cursor,
    })
}

#[derive(Default)]
struct SnippetCursorTracker {
    first_tabstop: Option<(u32, usize)>,
    final_tabstop: Option<usize>,
}

impl SnippetCursorTracker {
    fn record(&mut self, tabstop: u32, cursor: usize) {
        if tabstop == 0 {
            self.final_tabstop.get_or_insert(cursor);
        } else if self
            .first_tabstop
            .is_none_or(|(current, _)| tabstop < current)
        {
            self.first_tabstop = Some((tabstop, cursor));
        }
    }
}

fn expand_lsp_snippet(snippet: &str) -> Result<SnippetExpansion, String> {
    let characters = snippet.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(snippet.len());
    let mut tracker = SnippetCursorTracker::default();
    expand_lsp_snippet_fragment(&characters, &mut output, &mut tracker)?;
    let cursor = tracker
        .first_tabstop
        .map(|(_, cursor)| cursor)
        .or(tracker.final_tabstop)
        .unwrap_or_else(|| output.chars().count());
    Ok(SnippetExpansion {
        text: output,
        cursor,
    })
}

fn expand_lsp_snippet_fragment(
    characters: &[char],
    output: &mut String,
    tracker: &mut SnippetCursorTracker,
) -> Result<(), String> {
    let mut index = 0;
    while index < characters.len() {
        match characters[index] {
            '\\' if index + 1 < characters.len()
                && matches!(characters[index + 1], '$' | '}' | '\\') =>
            {
                output.push(characters[index + 1]);
                index += 2;
            }
            '$' if index + 1 < characters.len() && characters[index + 1].is_ascii_digit() => {
                let (tabstop, next) = parse_snippet_number(characters, index + 1);
                tracker.record(tabstop, output.chars().count());
                index = next;
            }
            '$' if index + 1 < characters.len() && characters[index + 1] == '{' => {
                let close = snippet_closing_brace(characters, index + 2)
                    .ok_or_else(|| "an LSP snippet placeholder is not closed".to_owned())?;
                expand_braced_snippet(&characters[index + 2..close], output, tracker)?;
                index = close + 1;
            }
            '$' if index + 1 < characters.len()
                && (characters[index + 1].is_ascii_alphabetic()
                    || characters[index + 1] == '_') =>
            {
                index += 2;
                while index < characters.len()
                    && (characters[index].is_ascii_alphanumeric() || characters[index] == '_')
                {
                    index += 1;
                }
            }
            character => {
                output.push(character);
                index += 1;
            }
        }
    }
    Ok(())
}

fn parse_snippet_number(characters: &[char], start: usize) -> (u32, usize) {
    let mut value = 0_u32;
    let mut index = start;
    while index < characters.len() && characters[index].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add(characters[index].to_digit(10).unwrap_or(0));
        index += 1;
    }
    (value, index)
}

fn snippet_closing_brace(characters: &[char], start: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut escaped = false;
    for (index, character) in characters.iter().copied().enumerate().skip(start) {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
        } else if character == '{' {
            depth += 1;
        } else if character == '}' {
            if depth == 0 {
                return Some(index);
            }
            depth -= 1;
        }
    }
    None
}

fn expand_braced_snippet(
    body: &[char],
    output: &mut String,
    tracker: &mut SnippetCursorTracker,
) -> Result<(), String> {
    if body
        .first()
        .is_some_and(|character| character.is_ascii_digit())
    {
        let (tabstop, after_number) = parse_snippet_number(body, 0);
        let cursor = output.chars().count();
        tracker.record(tabstop, cursor);
        match body.get(after_number) {
            None => {}
            Some(':') => {
                expand_lsp_snippet_fragment(&body[after_number + 1..], output, tracker)?;
            }
            Some('|') if body.last() == Some(&'|') => {
                let choice = first_snippet_choice(&body[after_number + 1..body.len() - 1]);
                output.extend(choice);
            }
            _ => return Err("an LSP snippet tabstop has an unsupported form".to_owned()),
        }
        return Ok(());
    }

    let separator = body.iter().position(|character| *character == ':');
    if let Some(separator) = separator {
        expand_lsp_snippet_fragment(&body[separator + 1..], output, tracker)?;
    } else if body
        .iter()
        .all(|character| character.is_ascii_alphanumeric() || *character == '_')
    {
        // Unknown variables have an empty value, as required by the snippet
        // fallback rules when no default is supplied.
    } else {
        return Err("an LSP snippet variable has an unsupported form".to_owned());
    }
    Ok(())
}

fn first_snippet_choice(characters: &[char]) -> Vec<char> {
    let mut choice = Vec::new();
    let mut escaped = false;
    for character in characters.iter().copied() {
        if escaped {
            choice.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ',' {
            break;
        } else {
            choice.push(character);
        }
    }
    if escaped {
        choice.push('\\');
    }
    choice
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
    let overlay_height = visible_rows.iter().map(|row| row.height).sum();

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
            for visible in visible_rows {
                let row = visible.row;
                let row_rect = Rect::from_min_size(
                    Pos2::new(overlay.left(), destination_top),
                    Vec2::new(overlay.width(), visible.height),
                );
                let response = ui.interact(
                    row_rect,
                    ui.id()
                        .with(("sticky-context-row", row.line, row.char_index)),
                    Sense::click(),
                );
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }

                let painter = ui.painter().with_clip_rect(row_rect.intersect(viewport));
                painter.galley(
                    Pos2::new(snapshot.galley_pos.x, row_rect.top() - visible.source_top),
                    Arc::clone(&snapshot.galley),
                    ui.visuals().text_color(),
                );
                if line_numbers {
                    painter.text(
                        Pos2::new(gutter.line_number_right, row_rect.top()),
                        egui::Align2::RIGHT_TOP,
                        row.line.to_string(),
                        theme::annotation_font(),
                        ui.visuals().weak_text_color(),
                    );
                }
                if let Some(target) = sticky_context_jump_target(row, response.clicked()) {
                    jump_target = Some(target);
                }
                destination_top += visible.height;
            }
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
        painter.text(
            Pos2::new(gutter.line_number_right, row_rect.top()),
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

fn add_workspace_nodes(
    builder: &mut TreeViewBuilder<'_, PathBuf>,
    nodes: &[WorkspaceNode],
    active: Option<&Path>,
    preview: Option<&Path>,
    context: &egui::Context,
    query: &str,
    git: &crate::git::editor::FileStatuses,
) {
    for node in nodes {
        if !workspace_node_matches_query(node, query) {
            continue;
        }
        let is_active = active.is_some_and(|path| path == node.path);
        let is_preview = preview.is_some_and(|path| path == node.path);
        let label = node.display_name().into_owned();
        let git_status = git.get(&node.path);
        if node.is_directory() {
            let color = workspace_entry_color(&node.path, true, false, context);
            let drop_directory = node.path.clone();
            let open = builder.node(
                NodeBuilder::dir(node.path.clone())
                    .default_open(active.is_some_and(|path| path.starts_with(&node.path)))
                    .icon(|ui| paint_tree_icon(ui, true))
                    .label_ui(move |ui| {
                        let color = workspace_entry_resolved_color(
                            color,
                            is_active,
                            ui.visuals().strong_text_color(),
                        );
                        let text = RichText::new(&label).color(color);
                        let response = ui.add(workspace_entry_label(text, is_active));
                        offer_folder_row_drop(ui, response.rect, &drop_directory);
                    }),
            );
            if open {
                add_workspace_nodes(
                    builder,
                    &node.children,
                    active,
                    preview,
                    context,
                    query,
                    git,
                );
            }
            builder.close_dir();
        } else if node.is_file() {
            let color = workspace_entry_color(&node.path, false, false, context);
            let hover_path = node.path.clone();
            let hover_kind = crate::document::preview_kind_for_path(&hover_path);
            builder.node(
                NodeBuilder::leaf(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, false))
                    .label_ui(move |ui| {
                        let row = ui.horizontal(|ui| {
                            let color = workspace_entry_resolved_color(
                                color,
                                is_active,
                                ui.visuals().strong_text_color(),
                            );
                            let text = RichText::new(&label).color(color);
                            ui.add(workspace_entry_label(text, is_active));
                            if let Some(status) = git_status {
                                ui.add(egui::Label::new(
                                    RichText::new(status.letter())
                                        .monospace()
                                        .strong()
                                        .color(status.color(context)),
                                ))
                                .on_hover_text(status.description());
                            }
                            if is_preview {
                                let blue = theme::palette(ui.ctx()).accent;
                                native_hover_text(
                                    static_icon(ui, UiIcon::Eye, blue),
                                    "Used for preview",
                                );
                            }
                        });
                        if let Some(parent) = hover_path.parent() {
                            offer_folder_row_drop(ui, row.response.rect, parent);
                        }
                        if let Some(kind) = hover_kind {
                            let hover_rect =
                                workspace_asset_hover_rect(row.response.rect, ui.clip_rect());
                            let hover_response = ui.interact(
                                hover_rect,
                                ui.id().with(("asset-row-hover", &hover_path)),
                                Sense::hover(),
                            );
                            offer_asset_hover(
                                &hover_response,
                                hover_rect,
                                hover_path.clone(),
                                kind,
                                TooltipPlacement::Right,
                            );
                        }
                    }),
            );
        } else if node.is_symlink() {
            let color = workspace_entry_color(&node.path, false, true, context);
            builder.node(
                NodeBuilder::leaf(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, false))
                    .label_ui(move |ui| {
                        let color = workspace_entry_resolved_color(
                            color,
                            is_active,
                            ui.visuals().strong_text_color(),
                        );
                        let text = RichText::new(format!("{label} (link)")).color(color);
                        ui.add(workspace_entry_label(text, is_active));
                    }),
            );
        }
    }
}

fn normalize_explorer_query(query: &str) -> String {
    query.trim().to_lowercase()
}

fn explorer_text_matches_query(text: &str, normalized_query: &str) -> bool {
    normalized_query.is_empty() || text.to_lowercase().contains(normalized_query)
}

fn explorer_path_matches_query(path: &Path, normalized_query: &str) -> bool {
    explorer_text_matches_query(&path.to_string_lossy(), normalized_query)
}

fn workspace_node_self_matches_query(node: &WorkspaceNode, normalized_query: &str) -> bool {
    explorer_text_matches_query(&node.display_name(), normalized_query)
        || explorer_path_matches_query(&node.relative_path, normalized_query)
}

fn workspace_node_matches_query(node: &WorkspaceNode, normalized_query: &str) -> bool {
    workspace_node_self_matches_query(node, normalized_query)
        || node
            .children
            .iter()
            .any(|child| workspace_node_matches_query(child, normalized_query))
}

fn open_matching_workspace_ancestors(
    state: &mut TreeViewState<PathBuf>,
    nodes: &[WorkspaceNode],
    normalized_query: &str,
) {
    for node in nodes.iter().filter(|node| node.is_directory()) {
        if workspace_node_matches_query(node, normalized_query) {
            state.set_openness(node.path.clone(), true);
            open_matching_workspace_ancestors(state, &node.children, normalized_query);
        }
    }
}

fn outline_entry_matches_query(
    entry: &crate::project_index::OutlineEntry,
    normalized_query: &str,
) -> bool {
    explorer_text_matches_query(&entry.title, normalized_query)
        || explorer_path_matches_query(&entry.path, normalized_query)
        || explorer_text_matches_query(&entry.line.to_string(), normalized_query)
}

fn symbol_entry_matches_query(
    entry: &crate::project_index::SymbolEntry,
    normalized_query: &str,
) -> bool {
    explorer_text_matches_query(&entry.name, normalized_query)
        || explorer_text_matches_query(entry.kind.label(), normalized_query)
        || explorer_path_matches_query(&entry.path, normalized_query)
        || explorer_text_matches_query(&entry.line.to_string(), normalized_query)
}

fn reference_entry_matches_query(
    entry: &crate::project_index::ReferenceEntry,
    normalized_query: &str,
) -> bool {
    explorer_text_matches_query(&entry.label, normalized_query)
        || explorer_path_matches_query(&entry.path, normalized_query)
        || explorer_text_matches_query(&entry.line.to_string(), normalized_query)
}

fn explorer_section_query_matches(
    snapshot: Option<&WorkspaceSnapshot>,
    index: &ProjectIndex,
    normalized_query: &str,
) -> [bool; EXPLORER_SECTION_SPECS.len()] {
    let mut matches = [
        snapshot.is_some_and(|snapshot| {
            snapshot
                .nodes
                .iter()
                .any(|node| workspace_node_matches_query(node, normalized_query))
        }),
        index
            .outline
            .iter()
            .any(|entry| outline_entry_matches_query(entry, normalized_query)),
        index
            .subfiles
            .iter()
            .any(|path| explorer_path_matches_query(path, normalized_query)),
        index
            .symbols
            .iter()
            .any(|entry| symbol_entry_matches_query(entry, normalized_query)),
        index
            .packages
            .iter()
            .any(|package| explorer_text_matches_query(package, normalized_query)),
        index
            .references
            .iter()
            .any(|entry| reference_entry_matches_query(entry, normalized_query)),
    ];
    if !matches.into_iter().any(|matched| matched) {
        // Keep one result surface visible so an empty search has a clear
        // outcome instead of presenting six closed section headers.
        matches[0] = true;
    }
    matches
}

fn workspace_entry_label(text: RichText, is_active: bool) -> egui::Label {
    let text = if is_active {
        text.font(theme::strong_ui_font()).strong()
    } else {
        text
    };
    theme::nonselectable_label(text)
}

fn workspace_asset_hover_rect(row: Rect, visible_panel: Rect) -> Rect {
    Rect::from_min_max(
        row.left_top(),
        Pos2::new(visible_panel.right().max(row.left()), row.bottom()),
    )
}

fn workspace_entry_resolved_color(
    category_color: Color32,
    is_active: bool,
    strong_text_color: Color32,
) -> Color32 {
    if is_active {
        strong_text_color
    } else {
        category_color
    }
}

fn workspace_entry_color(
    path: &Path,
    directory: bool,
    symlink: bool,
    context: &egui::Context,
) -> Color32 {
    if directory {
        return theme::palette(context).accent;
    }
    if symlink {
        return theme::syntax_palette(context).comment;
    }
    let syntax = theme::syntax_palette(context);
    let extension = path.extension().and_then(|extension| extension.to_str());
    if extension_matches(extension, &["typ"]) {
        return syntax.keyword;
    }
    if extension_matches(
        extension,
        &[
            "pdf", "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff",
        ],
    ) {
        return theme::palette(context).info;
    }
    if extension_matches(
        extension,
        &[
            "txt", "md", "markdown", "json", "jsonc", "toml", "yaml", "yml", "xml", "html", "htm",
            "css", "scss", "js", "jsx", "ts", "tsx", "rs", "py", "rb", "go", "java", "c", "h",
            "cc", "cpp", "hpp", "sh", "bash", "zsh", "fish", "sql", "csv", "tsv", "ini", "cfg",
            "conf", "log", "tex", "bib",
        ],
    ) {
        return syntax.plain;
    }
    syntax.comment
}

fn extension_matches(extension: Option<&str>, expected: &[&str]) -> bool {
    extension.is_some_and(|extension| {
        expected
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    })
}

const EXPLORER_SECTION_SPECS: [(&str, bool); 6] = [
    ("workspace-files", true),
    ("workspace-contents", true),
    ("workspace-subfiles", false),
    ("workspace-symbols", false),
    ("workspace-packages", false),
    ("workspace-references", false),
];

const EXPLORER_SECTION_MIN_BODY_HEIGHT: f32 = 44.0;
const EXPLORER_SECTION_RESIZE_HANDLE_HEIGHT: f32 = 5.0;

#[derive(Clone, Debug, PartialEq)]
struct ExplorerSectionLayout {
    weights: [f32; EXPLORER_SECTION_SPECS.len()],
}

impl Default for ExplorerSectionLayout {
    fn default() -> Self {
        Self {
            weights: [1.0; EXPLORER_SECTION_SPECS.len()],
        }
    }
}

impl ExplorerSectionLayout {
    fn body_heights(
        &self,
        open: [bool; EXPLORER_SECTION_SPECS.len()],
        available: f32,
    ) -> [f32; EXPLORER_SECTION_SPECS.len()] {
        let mut heights = [0.0; EXPLORER_SECTION_SPECS.len()];
        let open_count = open.iter().filter(|is_open| **is_open).count();
        if open_count == 0 {
            return heights;
        }

        let available = available.max(0.0);
        let minimum = EXPLORER_SECTION_MIN_BODY_HEIGHT.min(available / open_count as f32);
        let remainder = (available - minimum * open_count as f32).max(0.0);
        let weight_sum = self
            .weights
            .iter()
            .zip(open)
            .filter_map(|(weight, is_open)| {
                is_open.then_some(if weight.is_finite() && *weight > 0.0 {
                    *weight
                } else {
                    1.0
                })
            })
            .sum::<f32>()
            .max(f32::EPSILON);

        for (index, is_open) in open.into_iter().enumerate() {
            if is_open {
                let weight = self.weights[index];
                let weight = if weight.is_finite() && weight > 0.0 {
                    weight
                } else {
                    1.0
                };
                heights[index] = minimum + remainder * weight / weight_sum;
            }
        }
        heights
    }

    fn resize_after(
        &mut self,
        open: [bool; EXPLORER_SECTION_SPECS.len()],
        available: f32,
        upper_index: usize,
        requested_delta: f32,
    ) -> bool {
        if !requested_delta.is_finite() || requested_delta.abs() <= f32::EPSILON {
            return false;
        }
        let Some(lower_index) = next_open_explorer_section(open, upper_index) else {
            return false;
        };
        let mut heights = self.body_heights(open, available);
        let open_count = open.iter().filter(|is_open| **is_open).count();
        let minimum =
            EXPLORER_SECTION_MIN_BODY_HEIGHT.min(available.max(0.0) / open_count.max(1) as f32);
        let applied_delta = requested_delta.clamp(
            minimum - heights[upper_index],
            heights[lower_index] - minimum,
        );
        if applied_delta.abs() <= f32::EPSILON {
            return false;
        }
        heights[upper_index] += applied_delta;
        heights[lower_index] -= applied_delta;

        // Store relative preferences. Recomputing from these weights lets the
        // split scale with the panel while retaining the user's proportions.
        for (index, is_open) in open.into_iter().enumerate() {
            if is_open {
                self.weights[index] = (heights[index] - minimum).max(f32::EPSILON);
            }
        }
        true
    }
}

fn next_open_explorer_section(
    open: [bool; EXPLORER_SECTION_SPECS.len()],
    after: usize,
) -> Option<usize> {
    open.iter()
        .enumerate()
        .skip(after.saturating_add(1))
        .find_map(|(index, is_open)| (*is_open).then_some(index))
}

fn explorer_section_open_states(
    ui: &egui::Ui,
    filtered: bool,
    defaults: [bool; EXPLORER_SECTION_SPECS.len()],
) -> [bool; EXPLORER_SECTION_SPECS.len()] {
    std::array::from_fn(|index| {
        let (id_salt, _) = EXPLORER_SECTION_SPECS[index];
        if filtered {
            return defaults[index];
        }
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            explorer_section_state_id(ui, id_salt, false),
            defaults[index],
        )
        .is_open()
    })
}

fn explorer_section_layout_id(ui: &egui::Ui) -> egui::Id {
    ui.make_persistent_id("explorer-section-layout")
}

fn explorer_section_state_id(ui: &egui::Ui, id_salt: &'static str, filtered: bool) -> egui::Id {
    ui.make_persistent_id(("explorer-section", id_salt, filtered))
}

fn workspace_tree_state_id(ui: &egui::Ui, root: &Path, filtered: bool) -> egui::Id {
    ui.make_persistent_id(("workspace-tree", root, filtered))
}

#[cfg(test)]
fn explorer_section_body_height(ui: &egui::Ui) -> f32 {
    let defaults = std::array::from_fn(|index| EXPLORER_SECTION_SPECS[index].1);
    let open_sections = explorer_section_open_states(ui, false, defaults)
        .into_iter()
        .filter(|is_open| *is_open)
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

#[cfg(test)]
fn explorer_section(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    title: &'static str,
    default_open: bool,
    body_height: f32,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    let _ = explorer_section_resizable(
        ui,
        ExplorerSectionRenderSpec {
            id_salt,
            title,
            default_open,
            body_height,
            show_resize_handle: false,
            filtered: false,
        },
        add_body,
    );
}

#[derive(Debug, Clone, Copy)]
struct ExplorerSectionRenderSpec {
    id_salt: &'static str,
    title: &'static str,
    default_open: bool,
    body_height: f32,
    show_resize_handle: bool,
    filtered: bool,
}

fn explorer_section_resizable(
    ui: &mut egui::Ui,
    spec: ExplorerSectionRenderSpec,
    add_body: impl FnOnce(&mut egui::Ui),
) -> f32 {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        explorer_section_state_id(ui, spec.id_salt, spec.filtered),
        spec.default_open,
    );
    if spec.filtered {
        // Filtered results are transient and should always expose the sections
        // that contain matches without mutating the user's normal open state.
        state.set_open(spec.default_open);
    }
    let mut resize_delta = 0.0;
    theme::explorer_section_frame(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width().max(0.0));
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut title_clicked = false;
        let mut header = state.show_header(ui, |ui| {
            let response = ui.add_sized(
                [
                    ui.available_width().max(0.0),
                    METRICS.explorer.section_header_height,
                ],
                egui::Button::new(RichText::new(spec.title).strong()).frame(false),
            );
            title_clicked = response.clicked();
        });
        if title_clicked {
            header.toggle();
        }
        header.body_unindented(|ui| {
            let handle_height = if spec.show_resize_handle {
                EXPLORER_SECTION_RESIZE_HANDLE_HEIGHT.min(spec.body_height.max(0.0))
            } else {
                0.0
            };
            egui::ScrollArea::both()
                .id_salt((spec.id_salt, "scroll", spec.filtered))
                .max_width(ui.available_width().max(0.0))
                .max_height((spec.body_height - handle_height).max(0.0))
                .min_scrolled_width(0.0)
                .min_scrolled_height(0.0)
                .auto_shrink([false, false])
                .show(ui, add_body);
            if handle_height > 0.0 {
                let (rect, response) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width().max(0.0), handle_height),
                    Sense::drag(),
                );
                let response = response.on_hover_cursor(egui::CursorIcon::ResizeVertical);
                let stroke = if response.hovered() || response.dragged() {
                    Stroke::new(1.5, ui.visuals().widgets.hovered.fg_stroke.color)
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke
                };
                ui.painter().line_segment(
                    [
                        Pos2::new(rect.left(), rect.center().y),
                        Pos2::new(rect.right(), rect.center().y),
                    ],
                    stroke,
                );
                resize_delta = response.drag_delta().y;
            }
        });
    });
    resize_delta
}

struct ExplorerProjectSectionsSpec<'a> {
    root: &'a Path,
    index: &'a ProjectIndex,
    query: &'a str,
    filtered: bool,
    section_defaults: [bool; EXPLORER_SECTION_SPECS.len()],
    open_sections: [bool; EXPLORER_SECTION_SPECS.len()],
    body_heights: [f32; EXPLORER_SECTION_SPECS.len()],
}

#[derive(Default)]
struct ExplorerProjectSectionsOutcome {
    resize_request: Option<(usize, f32)>,
    open_package_manager: bool,
    target: Option<(PathBuf, usize)>,
}

fn show_project_index_sections(
    ui: &mut egui::Ui,
    spec: ExplorerProjectSectionsSpec<'_>,
) -> ExplorerProjectSectionsOutcome {
    let ExplorerProjectSectionsSpec {
        root,
        index,
        query,
        filtered,
        section_defaults,
        open_sections,
        body_heights,
    } = spec;
    let mut outcome = ExplorerProjectSectionsOutcome::default();
    let resize_delta = explorer_section_resizable(
        ui,
        ExplorerSectionRenderSpec {
            id_salt: "workspace-contents",
            title: "Contents",
            default_open: section_defaults[1],
            body_height: body_heights[1],
            show_resize_handle: next_open_explorer_section(open_sections, 1).is_some(),
            filtered,
        },
        |ui| {
            let mut entries = index
                .outline
                .iter()
                .filter(|entry| outline_entry_matches_query(entry, query))
                .peekable();
            if entries.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else {
                        "No headings"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
                return;
            }
            for entry in entries {
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
                    outcome.target = Some((entry.path.clone(), entry.line));
                }
            }
        },
    );
    if resize_delta.abs() > f32::EPSILON {
        outcome.resize_request = Some((1, resize_delta));
    }

    let resize_delta = explorer_section_resizable(
        ui,
        ExplorerSectionRenderSpec {
            id_salt: "workspace-subfiles",
            title: "Subfiles",
            default_open: section_defaults[2],
            body_height: body_heights[2],
            show_resize_handle: next_open_explorer_section(open_sections, 2).is_some(),
            filtered,
        },
        |ui| {
            let mut paths = index
                .subfiles
                .iter()
                .filter(|path| explorer_path_matches_query(path, query))
                .peekable();
            if paths.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else {
                        "No included files"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
                return;
            }
            for path in paths {
                let label = project_relative_path(root, path);
                let response = explorer_index_row(ui, &label, None, 0.0);
                if native_hover_text(response, path.display().to_string()).clicked() {
                    outcome.target = Some((path.clone(), 1));
                }
            }
        },
    );
    if resize_delta.abs() > f32::EPSILON {
        outcome.resize_request = Some((2, resize_delta));
    }

    let resize_delta = explorer_section_resizable(
        ui,
        ExplorerSectionRenderSpec {
            id_salt: "workspace-symbols",
            title: "Symbols",
            default_open: section_defaults[3],
            body_height: body_heights[3],
            show_resize_handle: next_open_explorer_section(open_sections, 3).is_some(),
            filtered,
        },
        |ui| {
            let mut symbols = index
                .symbols
                .iter()
                .filter(|entry| symbol_entry_matches_query(entry, query))
                .peekable();
            if symbols.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else {
                        "No definitions or functions"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
                return;
            }
            for symbol in symbols {
                let kind = symbol.kind.label();
                let location = format!(
                    "{}:{} · {kind}",
                    project_relative_path(root, &symbol.path),
                    symbol.line
                );
                let response = explorer_index_row(ui, &symbol.name, Some(kind), 0.0);
                if native_hover_text(response, location).clicked() {
                    outcome.target = Some((symbol.path.clone(), symbol.line));
                }
            }
        },
    );
    if resize_delta.abs() > f32::EPSILON {
        outcome.resize_request = Some((3, resize_delta));
    }

    let resize_delta = explorer_section_resizable(
        ui,
        ExplorerSectionRenderSpec {
            id_salt: "workspace-packages",
            title: "Packages",
            default_open: section_defaults[4],
            body_height: body_heights[4],
            show_resize_handle: next_open_explorer_section(open_sections, 4).is_some(),
            filtered,
        },
        |ui| {
            if ui.button("Browse packages…").clicked() {
                outcome.open_package_manager = true;
            }
            ui.separator();
            let mut packages = index
                .packages
                .iter()
                .filter(|package| explorer_text_matches_query(package, query))
                .peekable();
            if packages.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else {
                        "No packages"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
            } else {
                for package in packages {
                    explorer_index_row(ui, package, None, 0.0);
                }
            }
        },
    );
    if resize_delta.abs() > f32::EPSILON {
        outcome.resize_request = Some((4, resize_delta));
    }

    let resize_delta = explorer_section_resizable(
        ui,
        ExplorerSectionRenderSpec {
            id_salt: "workspace-references",
            title: "Tags and references",
            default_open: section_defaults[5],
            body_height: body_heights[5],
            show_resize_handle: next_open_explorer_section(open_sections, 5).is_some(),
            filtered,
        },
        |ui| {
            let mut references = index
                .references
                .iter()
                .filter(|entry| reference_entry_matches_query(entry, query))
                .peekable();
            if references.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else {
                        "No tags or references"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
                return;
            }
            for reference in references {
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
                    outcome.target = Some((reference.path.clone(), reference.line));
                }
            }
        },
    );
    if resize_delta.abs() > f32::EPSILON {
        outcome.resize_request = Some((5, resize_delta));
    }
    outcome
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
    result
        .map(|_| ())
        .map_err(|error| format!("Could not open the link in the system browser: {error}"))
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
    (kind.is_typst() && !elapsed.is_zero())
        .then(|| format!("{:.0} ms", elapsed.as_secs_f64() * 1000.0))
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
        .width(230.0)
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

fn source_editor_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-source-editor")
}

fn focused_input_viewport(context: &egui::Context) -> egui::ViewportId {
    let current = context.viewport_id();
    // Only consider this document window and its own overlays. Every
    // `EditorApp` is rendered once per pass, so scanning all focused viewports
    // would let the primary editor steal shortcuts typed into a secondary
    // document before that document's callback runs.
    let owned = [
        current,
        scoped_child_viewport_id(context, "tiptoptyp-packages"),
        scoped_child_viewport_id(context, "tiptoptyp-git"),
        scoped_child_viewport_id(context, "tiptoptyp-git-chunk"),
        scoped_child_viewport_id(context, "tiptoptyp-table-editor-overlay"),
        scoped_child_viewport_id(context, "tiptoptyp-rename-overlay"),
        scoped_child_viewport_id(context, "tiptoptyp-workspace-chooser"),
        scoped_child_viewport_id(context, "tiptoptyp-modal-overlay"),
        scoped_child_viewport_id(context, "tiptoptyp-settings"),
        scoped_child_viewport_id(context, "tiptoptyp-typst-overrides"),
        scoped_child_viewport_id(context, "asset-hover-overlay"),
        scoped_child_viewport_id(context, "tiptoptyp-popup-overlay"),
        scoped_child_viewport_id(context, "diagnostic-tooltip-overlay"),
    ];
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

fn native_hover_tooltip_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-native-hover-tooltip")
}

fn asset_hover_candidate_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-asset-hover-candidate")
}

fn current_asset_hover_candidate(context: &egui::Context) -> Option<AssetHoverCandidate> {
    // Derive the viewport-scoped ID before entering the data lock. Calling
    // `viewport_id()` from inside `Context::data` would re-enter the same lock
    // and deadlock the first UI frame.
    let id = asset_hover_candidate_id(context);
    context.data(|data| data.get_temp::<AssetHoverCandidate>(id))
}

fn clear_asset_hover_candidate(context: &egui::Context) {
    let id = asset_hover_candidate_id(context);
    context.data_mut(|data| data.remove::<AssetHoverCandidate>(id));
}

fn clear_native_hover_overlay(context: &egui::Context) {
    let id = native_hover_tooltip_id(context);
    context.data_mut(|data| data.remove::<HoverTooltipOverlay>(id));
}

fn asset_hover_timing_id(context: &egui::Context) -> egui::Id {
    native_hover_tooltip_id(context).with("asset-hover-timing")
}

fn tooltip_geometry_id(context: &egui::Context) -> egui::Id {
    native_hover_tooltip_id(context).with("geometry")
}

fn tooltip_interaction_id(context: &egui::Context) -> egui::Id {
    native_hover_tooltip_id(context).with("interaction")
}

fn offer_asset_hover(
    response: &egui::Response,
    origin: Rect,
    path: PathBuf,
    kind: DocumentKind,
    placement: TooltipPlacement,
) {
    if native_tooltip_handoff_blocks(&response.ctx, origin) {
        return;
    }
    let Some(opacity) = hover_opacity(response, asset_hover_timing_id(&response.ctx)) else {
        return;
    };
    let anchor = match placement {
        TooltipPlacement::Below => {
            origin.left_bottom() + egui::vec2(0.0, METRICS.editor.tooltip_gap)
        }
        TooltipPlacement::Right => {
            origin.right_center() + egui::vec2(METRICS.editor.tooltip_gap, 0.0)
        }
    };
    let candidate = AssetHoverCandidate {
        origin,
        anchor,
        placement,
        path,
        kind,
        opacity,
    };
    let id = asset_hover_candidate_id(&response.ctx);
    response
        .ctx
        .data_mut(|data| data.insert_temp(id, candidate));
}

fn asset_thumbnail_result_matches(
    hover: Option<&AssetHoverState>,
    result: &AssetThumbnailResult,
) -> bool {
    hover.is_some_and(|hover| {
        hover.token == result.token && hover.path == result.path && hover.kind == result.kind
    })
}

fn asset_tooltip_identity(origin: Rect, path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    for coordinate in [origin.min.x, origin.min.y, origin.max.x, origin.max.y] {
        coordinate.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

fn fit_asset_preview_size(source: [usize; 2], bounds: Vec2) -> Vec2 {
    let source = Vec2::new(source[0].max(1) as f32, source[1].max(1) as f32);
    let bounds = Vec2::new(bounds.x.max(1.0), bounds.y.max(1.0));
    let scale = (bounds.x / source.x).min(bounds.y / source.y).min(1.0);
    (source * scale).max(Vec2::splat(1.0))
}

fn asset_hover_card_size(
    content: &AssetHoverContent,
    viewport_size: Vec2,
    frame_margin: Vec2,
    edge: f32,
) -> Vec2 {
    let desired = match content {
        AssetHoverContent::Loading => ASSET_HOVER_LOADING_SIZE,
        AssetHoverContent::Error(_) => ASSET_HOVER_ERROR_SIZE,
        AssetHoverContent::Ready { source_size, .. } => {
            let image = fit_asset_preview_size(*source_size, ASSET_HOVER_CARD_MAX_IMAGE);
            Vec2::new(image.x + frame_margin.x, image.y + frame_margin.y)
        }
    };
    let available = (viewport_size - Vec2::splat(edge.max(0.0) * 2.0)).max(Vec2::splat(1.0));
    desired.min(available).max(Vec2::splat(1.0))
}

fn show_asset_hover_contents(
    ui: &mut egui::Ui,
    path: &Path,
    _kind: DocumentKind,
    content: &AssetHoverContent,
    content_size: Vec2,
) {
    ui.set_min_size(content_size);
    ui.set_max_size(content_size);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    match content {
        AssetHoverContent::Loading => {
            ui.vertical_centered(|ui| {
                ui.add_space((content_size.y - 44.0).max(0.0) * 0.5);
                ui.spinner();
                ui.label(format!("Loading preview for {file_name}…"));
            });
        }
        AssetHoverContent::Error(error) => {
            ui.add_sized(
                [content_size.x, METRICS.popup.tooltip_title_height],
                egui::Label::new(RichText::new(file_name).strong())
                    .selectable(true)
                    .truncate(),
            );
            ui.separator();
            ui.add(egui::Label::new(error).selectable(true).wrap());
        }
        AssetHoverContent::Ready {
            texture,
            source_size,
        } => {
            let image_size = fit_asset_preview_size(*source_size, content_size);
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(image_size)
                        .alt_text(format!("Preview of {file_name}")),
                );
            });
        }
    }
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
    let dismissed_id = viewport_scoped_id(context, "dismissed-tooltip-origin");
    if context.data(|data| {
        data.get_temp::<Rect>(dismissed_id)
            .is_some_and(|origin| origin.intersects(candidate_origin))
    }) {
        return true;
    }
    let active = native_tooltip_handoff_active(context, false);
    let geometry_id = tooltip_geometry_id(context);
    let active_origin = context.data(|data| {
        data.get_temp::<TooltipGeometry>(geometry_id)
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

fn hover_runtime_config(context: &egui::Context) -> HoverRuntimeConfig {
    let id = hover_runtime_config_id(context);
    context.data(|data| {
        data.get_temp::<HoverRuntimeConfig>(id)
            .unwrap_or(HoverRuntimeConfig {
                delay: Duration::from_millis(DEFAULT_HOVER_DELAY_MS),
                fade: Duration::from_millis(DEFAULT_HOVER_FADE_MS),
            })
    })
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
    let force_id = viewport_scoped_id(&response.ctx, "force-pointer-tooltip");
    if response
        .ctx
        .data(|data| data.get_temp::<bool>(force_id).unwrap_or(false))
    {
        return Some(1.0);
    }
    let now = response.ctx.input(|input| input.time);
    let config = hover_runtime_config(&response.ctx);
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
    placement: TooltipPlacement,
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
    let root_local_card = place_native_tooltip_card(
        Rect::from_min_size(Pos2::ZERO, window_rect.size()),
        origin,
        anchor,
        Vec2::new(width, height),
        placement,
        METRICS.popup.viewport_edge,
    );
    let position = window_rect.min + root_local_card.min.to_vec2();
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
    let now = context.input(|input| input.time);
    let previous = context.data(|data| {
        data.get_temp::<TooltipGeometry>(geometry_id)
            .filter(|geometry| geometry.identity == identity)
    });
    let fade = continue_tooltip_fade(
        opacity,
        previous.map(|geometry| geometry.fade),
        now,
        hover_runtime_config(context).fade,
    );
    if fade.opacity < 1.0 {
        context.request_repaint_after(METRICS.motion.animation_frame);
    }
    context.data_mut(|data| {
        data.insert_temp(
            geometry_id,
            TooltipGeometry {
                identity,
                origin,
                card: root_local_card,
                fade,
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
        ui.set_opacity(fade.opacity);
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
    let job = tooltip_code_job(
        highlighter,
        typst_highlighter,
        source,
        &token,
        dark_mode,
        theme::syntax_palette(ui.ctx()),
    );
    if let Some(job) = job {
        ui.add(egui::Label::new(job).selectable(true).wrap());
    } else {
        let color = theme::syntax_palette(ui.ctx()).plain;
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
    palette: theme::SyntaxPalette,
) -> Option<egui::text::LayoutJob> {
    if matches!(token, "typ" | "typst" | "typc") {
        typst_highlighter.set_styles(ResolvedTypstStyles::resolve(
            palette,
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
                theme::syntax_palette(ui.ctx()),
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
        if gap <= f32::EPSILON || pointer.x < origin.right() || pointer.x > card.left() {
            return false;
        }
        let t = ((pointer.x - origin.right()) / gap).clamp(0.0, 1.0);
        let top = egui::lerp(origin.top()..=card.top(), t);
        let bottom = egui::lerp(origin.bottom()..=card.bottom(), t);
        pointer.y >= top && pointer.y <= bottom
    } else if card.right() <= origin.left() {
        let gap = origin.left() - card.right();
        if gap <= f32::EPSILON || pointer.x < card.right() || pointer.x > origin.left() {
            return false;
        }
        let t = ((origin.left() - pointer.x) / gap).clamp(0.0, 1.0);
        let top = egui::lerp(card.top()..=origin.top(), t);
        let bottom = egui::lerp(card.bottom()..=origin.bottom(), t);
        pointer.y >= top && pointer.y <= bottom
    } else if card.top() >= origin.bottom() {
        let gap = card.top() - origin.bottom();
        if gap <= f32::EPSILON || pointer.y < origin.bottom() || pointer.y > card.top() {
            return false;
        }
        let t = ((pointer.y - origin.bottom()) / gap).clamp(0.0, 1.0);
        let left = egui::lerp(origin.left()..=card.left(), t);
        let right = egui::lerp(origin.right()..=card.right(), t);
        pointer.x >= left && pointer.x <= right
    } else if card.bottom() <= origin.top() {
        let gap = origin.top() - card.bottom();
        if gap <= f32::EPSILON || pointer.y < card.bottom() || pointer.y > origin.top() {
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

fn place_native_tooltip_card(
    viewport: Rect,
    origin: Rect,
    anchor: Pos2,
    size: Vec2,
    placement: TooltipPlacement,
    edge: f32,
) -> Rect {
    let edge = edge.max(0.0);
    let min_x = (viewport.left() + edge).min(viewport.center().x);
    let min_y = (viewport.top() + edge).min(viewport.center().y);
    let max_x = (viewport.right() - edge - size.x).max(min_x);
    let max_y = (viewport.bottom() - edge - size.y).max(min_y);

    let (x, y) = match placement {
        TooltipPlacement::Below => {
            let gap = (anchor.y - origin.bottom()).max(0.0);
            let below = anchor.y;
            let above = origin.top() - gap - size.y;
            let y = if (min_y..=max_y).contains(&below) {
                below
            } else if (min_y..=max_y).contains(&above) {
                above
            } else {
                let room_below = (viewport.bottom() - edge - origin.bottom() - gap).max(0.0);
                let room_above = (origin.top() - gap - viewport.top() - edge).max(0.0);
                if room_above > room_below {
                    above.clamp(min_y, max_y)
                } else {
                    below.clamp(min_y, max_y)
                }
            };
            (anchor.x.clamp(min_x, max_x), y)
        }
        TooltipPlacement::Right => {
            let gap = (anchor.x - origin.right()).max(0.0);
            let right = anchor.x;
            let left = origin.left() - gap - size.x;
            let x = if (min_x..=max_x).contains(&right) {
                right
            } else if (min_x..=max_x).contains(&left) {
                left
            } else {
                let room_right = (viewport.right() - edge - origin.right() - gap).max(0.0);
                let room_left = (origin.left() - gap - viewport.left() - edge).max(0.0);
                if room_left > room_right {
                    left.clamp(min_x, max_x)
                } else {
                    right.clamp(min_x, max_x)
                }
            };
            (x, anchor.y.clamp(min_y, max_y))
        }
    };
    Rect::from_min_size(Pos2::new(x, y), size)
}

fn continue_tooltip_fade(
    sampled_opacity: f32,
    previous: Option<TooltipFadeState>,
    now: f64,
    duration: Duration,
) -> TooltipFadeState {
    let sampled_opacity = sampled_opacity.clamp(0.0, 1.0);
    let updated_at = previous.map_or(now, |previous| previous.updated_at.max(now));
    let opacity = if duration.is_zero() {
        1.0
    } else if let Some(previous) = previous {
        let elapsed = (now - previous.updated_at).max(0.0) as f32;
        let continued = previous.opacity + elapsed / duration.as_secs_f32();
        sampled_opacity.max(continued).clamp(0.0, 1.0)
    } else {
        sampled_opacity
    };
    TooltipFadeState {
        opacity,
        updated_at,
    }
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

#[derive(Debug, Clone, Copy)]
struct CommandAvailability {
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
        let enabled = availability.allows(spec.requirement);
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
        "Stop using for preview"
    } else {
        "Use for preview"
    };
    if menu_item_enabled(ui, is_typst, preview_label, None).clicked() {
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

fn show_popup_contents<R>(
    ui: &mut egui::Ui,
    size: Vec2,
    generation: u64,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    // Areas remember their last size. Explicitly update both dimensions so
    // opening a taller menu cannot inherit the previous menu's scroll bounds.
    ui.set_width(size.x);
    ui.set_height(size.y);
    egui::ScrollArea::vertical()
        .id_salt(app_popup_scroll_id(generation))
        .max_height(size.y)
        .show(ui, body)
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

fn editor_context_menu_size(
    has_link: bool,
    can_edit_table: bool,
    can_format: bool,
    style: &egui::Style,
) -> Vec2 {
    let extra = usize::from(has_link) + usize::from(can_edit_table) + usize::from(can_format);
    menu_popup_size(METRICS.menu.editor_width, 8 + extra, 1 + extra, style)
}

fn command_popup_size(menu: CommandMenu, style: &egui::Style) -> Vec2 {
    let specs = command_specs(menu).collect::<Vec<_>>();
    let separators = specs
        .windows(2)
        .filter(|pair| pair[0].section != pair[1].section)
        .count();
    menu_popup_size(330.0, specs.len(), separators, style)
}

fn menu_popup_size(width: f32, rows: usize, separators: usize, style: &egui::Style) -> Vec2 {
    let spacing = theme::SPACE.small;
    let rows = rows as f32 * (METRICS.menu.row_height + spacing);
    let separators = separators as f32 * (theme::SPACE.control + spacing);
    Vec2::new(
        width,
        rows + separators + theme::menu_card_frame(style).total_margin().sum().y + 2.0,
    )
}

fn workspace_context_menu_size(is_file: bool, style: &egui::Style) -> Vec2 {
    // A file has three copy actions where a directory has one.
    menu_popup_size(
        METRICS.menu.workspace_width,
        if is_file { 9 } else { 7 },
        1,
        style,
    )
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

#[derive(Default)]
struct PackageBrowserAction {
    copied: Option<String>,
    open_link: Option<String>,
    uninstall: Option<crate::package_catalog::PackageInstallation>,
}

fn show_package_browser_ui(
    ui: &mut egui::Ui,
    query: &mut String,
    filter: &mut PackageFilter,
    load: Option<&PackageCatalogLoad>,
    loading: bool,
    action: &mut PackageBrowserAction,
) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(query)
                .hint_text("Search package names, descriptions, authors, or versions")
                .desired_width(f32::INFINITY),
        );
    });
    ui.horizontal_wrapped(|ui| {
        theme::apply_compact_control_spacing(ui);
        for choice in PackageFilter::ALL {
            ui.selectable_value(filter, choice, choice.label());
        }
        if loading {
            ui.spinner();
            ui.label("Refreshing local and published packages…");
        }
    });
    ui.separator();

    let Some(load) = load else {
        ui.vertical_centered(|ui| {
            ui.add_space(theme::SPACE.content);
            if loading {
                ui.spinner();
                ui.label("Inspecting Typst package directories and registry…");
            } else {
                ui.label(RichText::new("No package catalog has been loaded").weak());
            }
        });
        return;
    };

    if let Some(error) = &load.official_index_error {
        ui.colored_label(
            warning_color(ui.ctx()),
            format!("Published registry unavailable: {error}. Local packages are still shown."),
        );
    }
    if !load.warnings.is_empty() {
        egui::CollapsingHeader::new(format!(
            "{} local package scan warning{}",
            load.warnings.len(),
            if load.warnings.len() == 1 { "" } else { "s" }
        ))
        .show(ui, |ui| {
            for warning in &load.warnings {
                ui.label(format!("{}: {}", warning.path.display(), warning.message));
            }
        });
    }

    let packages = load
        .catalog
        .filtered(query)
        .into_iter()
        .filter(|package| filter.allows(package))
        .collect::<Vec<_>>();
    ui.label(
        RichText::new(format!(
            "{} package{}",
            packages.len(),
            if packages.len() == 1 { "" } else { "s" }
        ))
        .size(theme::TYPE.supporting)
        .weak(),
    );
    egui::ScrollArea::vertical()
        .id_salt("package-catalog-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for package in packages {
                let version = package
                    .latest_available
                    .or_else(|| package.latest_installed());
                let release = package.display_release();
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        let identity = version.map_or_else(
                            || format!("@{}/{}", package.namespace, package.name),
                            |version| format!("@{}/{}:{version}", package.namespace, package.name),
                        );
                        ui.label(RichText::new(&identity).monospace().strong());
                        if package.is_installed() {
                            ui.label(RichText::new("Installed").color(success_color(ui.ctx())));
                        }
                        if package.latest_available.is_some() {
                            ui.label(RichText::new("Published").weak());
                        }
                        if package.has_update() {
                            ui.label(
                                RichText::new("Update available").color(warning_color(ui.ctx())),
                            );
                        }
                        if let Some(website) = release.and_then(|release| {
                            release
                                .metadata
                                .homepage
                                .as_deref()
                                .or(release.metadata.repository.as_deref())
                        }) && ui.button("Website").clicked()
                        {
                            action.open_link = Some(website.to_owned());
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(version.is_some(), egui::Button::new("Copy import"))
                                .clicked()
                                && let Some(version) = version
                            {
                                action.copied = Some(format!(
                                    "#import \"@{}/{}:{version}\": *",
                                    package.namespace, package.name
                                ));
                            }
                        });
                    });
                    if let Some(description) =
                        release.and_then(|release| release.metadata.description.as_deref())
                    {
                        ui.label(description);
                    }
                    if let Some(installed) = package.latest_installed() {
                        ui.label(
                            RichText::new(format!("Latest local version: {installed}"))
                                .size(theme::TYPE.supporting)
                                .weak(),
                        );
                    }
                    for local_release in package.installed_releases() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("Installed {}", local_release.version))
                                    .monospace()
                                    .strong(),
                            );
                            if local_release.available {
                                ui.label(RichText::new("Published release").weak());
                            }
                        });
                        for installation in &local_release.installations {
                            if ui
                                .add_enabled(
                                    !loading,
                                    egui::Button::new(format!(
                                        "Uninstall {}…",
                                        local_release.version
                                    )),
                                )
                                .clicked()
                            {
                                action.uninstall = Some(installation.clone());
                            }
                            let root_kind = match installation.root.kind {
                                PackageRootKind::Data => "data",
                                PackageRootKind::Cache => "cache",
                            };
                            let root_kind = if installation.root.custom {
                                format!("custom {root_kind}")
                            } else {
                                root_kind.to_owned()
                            };
                            ui.label(
                                RichText::new(format!(
                                    "{root_kind}: {}",
                                    installation.package_path.display()
                                ))
                                .size(theme::TYPE.supporting)
                                .monospace()
                                .weak(),
                            );
                        }
                    }
                });
                ui.add_space(theme::SPACE.tight);
            }
        });
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

fn show_shortcut_editor_window(
    context: &egui::Context,
    visible: &mut bool,
    query: &mut String,
    capture: &mut Option<ShortcutAction>,
    notice: &mut Option<String>,
    settings: &mut AppSettings,
) {
    if !*visible {
        *capture = None;
        return;
    }

    let mut open = *visible;
    egui::Window::new("Keyboard shortcuts")
        .id(viewport_scoped_id(context, "keyboard-shortcut-editor"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([620.0, 540.0])
        .min_size([440.0, 300.0])
        .show(context, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(query)
                        .hint_text("Search actions or groups")
                        .desired_width(f32::INFINITY),
                );
                if ui.button("Reset all").clicked() {
                    settings.shortcut_overrides.reset_all();
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
            let bindings = settings.effective_shortcuts();
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
                            format!("{} {} {}", action.group(), action.label(), action.id())
                                .to_lowercase();
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
                                settings.shortcut_overrides.set(action, None);
                                *capture = None;
                                *notice = Some(format!("Disabled {}", action.label()));
                            }
                            if ui
                                .add_enabled(
                                    settings.shortcut_overrides.get(action).is_some(),
                                    egui::Button::new("Reset"),
                                )
                                .clicked()
                            {
                                settings.shortcut_overrides.reset(action);
                                *notice = Some(format!("Restored {}", action.label()));
                            }
                        });
                    }
                    if shown == 0 {
                        ui.label(RichText::new("No shortcut actions found").weak());
                    }
                });
        });
    *visible = open;
    if !open {
        *capture = None;
    }
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

fn atomic_write(
    path: &Path,
    contents: &[u8],
) -> Result<crate::private_workspace::WriteDurability, String> {
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
mod tests;
