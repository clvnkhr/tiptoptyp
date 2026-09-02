use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fs,
    future::Future,
    hash::{Hash, Hasher},
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, mpsc},
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align, Color32, ColorImage, FontFamily, FontId, KeyboardShortcut, Layout, Modifiers,
    Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2,
    text::{CCursor, CCursorRange},
};
use egui_ltreeview::{Action as TreeAction, NodeBuilder, TreeView, TreeViewBuilder};
use rfd::{AsyncFileDialog, FileHandle};

use crate::{
    asset::{AssetLoader, LoadedAsset},
    compiler::{CompileRequest, Compiler, PreviewLink, PreviewPage},
    diagnostics::{
        Diagnostic, DiagnosticLocation, DiagnosticSeverity, DiagnosticSource,
        parse_typst_short_output,
    },
    document::DocumentKind,
    generic_highlight::GenericSyntaxHighlighter,
    highlight::SyntaxHighlighter,
    preview::{
        PAGE_MARGIN, dark_preview_rgba, page_stack_geometry, stack_height, visible_page,
        zoom_anchored_offset,
    },
    search::{SearchState, find, find_all},
    settings::{
        AppSettings, DocumentTheme, InterfaceTheme, PreviewPreference, SourcePreviewTrigger,
        ToolMode, ToolPreference,
    },
    tinymist::{
        DiagnosticSeverity as TinymistDiagnosticSeverity, Generation, InvertColors, LspPosition,
        LspRange, LspTextEdit, TextDocument, TinymistConfig, TinymistDiagnostic, TinymistEvent,
        TinymistSidecar,
    },
    toolchain::{ToolKind, ToolOrigin, ToolResolution, resolve_tool},
    workspace::{WorkspaceNode, WorkspaceTree},
};

const COMPILE_DEBOUNCE: Duration = Duration::from_millis(60);
const WORKSPACE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const TOOLBAR_HEIGHT: f32 = 30.0;
const STATUS_HEIGHT: f32 = 24.0;
const PANEL_HEADER_HEIGHT: f32 = 28.0;
const SETTINGS_PANEL_WIDTH: f32 = 620.0;
const DIAGNOSTIC_TOOLTIP_WIDTH: f32 = 360.0;
const MIN_PREVIEW_ZOOM: f32 = 0.2;
const MAX_PREVIEW_ZOOM: f32 = 6.0;
const EDITOR_ATTENTION_DURATION: Duration = Duration::from_millis(420);

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

#[derive(Debug, Clone, Copy)]
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
struct DiagnosticTooltipOverlay {
    anchor: Pos2,
    severity: DiagnosticSeverity,
    detail: String,
}

#[derive(Debug, Clone, Default)]
struct HoverTooltipOverlay {
    anchor: Pos2,
    detail: String,
}

#[derive(Debug, Clone)]
struct EditorSnapshot {
    source: String,
    cursor: CCursorRange,
}

struct PendingExport {
    path: PathBuf,
    revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPickerTarget {
    Typst,
    Tinymist,
}

struct PendingToolPicker {
    target: ToolPickerTarget,
    future: Pin<Box<dyn Future<Output = Option<FileHandle>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentDialogTarget {
    OpenFile,
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
    Open,
    OpenFolder,
    Save,
    SaveAs,
    ExportPdf,
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
    SaveThen(Box<DeferredDocumentAction>),
    ForceSave(PathBuf),
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
        action: DeferredDocumentAction,
    },
    Overwrite {
        message: String,
        path: PathBuf,
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

    view_mode: ViewMode,
    filesystem_visible: bool,
    problems_visible: bool,
    settings_visible: bool,
    settings: AppSettings,
    pending_settings: Option<AppSettings>,
    applied_preview_preference: PreviewPreference,
    applied_typst_preference: ToolPreference,
    applied_tinymist_preference: ToolPreference,
    tool_refresh_requested: bool,
    typst_tool: ToolResolution,
    tinymist_tool: ToolResolution,
    workspace_root: PathBuf,
    workspace: Option<WorkspaceTree>,
    workspace_error: Option<String>,
    next_workspace_refresh: Instant,

    find_visible: bool,
    replace_visible: bool,
    find_query: String,
    replacement: String,
    search: SearchState,
    focus_find: bool,
    pending_editor_selection: Option<Range<usize>>,
    editor_attention: Option<EditorAttention>,
    reset_editor_history: bool,
    editor_undo: Vec<EditorSnapshot>,
    editor_redo: Vec<EditorSnapshot>,
    autosave_deadline: Option<Instant>,
    diagnostic_tooltip: Option<DiagnosticTooltipOverlay>,
    app_popup: Option<AppPopup>,
    app_popup_had_focus: bool,
    pending_app_popup_action: Option<AppPopupAction>,
    app_modal: Option<AppModal>,
    pending_document_action: Option<DeferredDocumentAction>,
    post_save_action: Option<DeferredDocumentAction>,
    rename_dialog: Option<RenameDialog>,

    pending_export: Option<PendingExport>,
    pending_export_dialog: Option<Pin<Box<dyn Future<Output = Option<FileHandle>>>>>,
    pending_tool_picker: Option<PendingToolPicker>,
    pending_document_dialog: Option<PendingDocumentDialog>,
    notice: Option<Notice>,
    last_title: String,
    allow_close: bool,

    tinymist: TinymistSidecar,
    tinymist_generation: Option<Generation>,
    tinymist_uri: Option<String>,
    tinymist_url: Option<String>,
    tinymist_lsp_ready: bool,
    tinymist_state: ServiceState,
    webview_state: ServiceState,

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
    pub fn new(context: &eframe::CreationContext<'_>, initial_path: Option<PathBuf>) -> Self {
        let settings = AppSettings::load(context.storage);
        let launch_directory = initial_path
            .as_deref()
            .filter(|path| path.is_dir())
            .map(Path::to_path_buf)
            .or_else(|| {
                initial_path
                    .as_deref()
                    .filter(|path| path.is_file())
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            })
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let workspace_root = if initial_path.as_deref().is_some_and(Path::is_dir) {
            canonical_or_absolute(&launch_directory)
        } else {
            canonical_or_absolute(&discover_project_root(&launch_directory))
        };
        let typst_tool = resolve_tool(ToolKind::Typst, &settings.typst);
        let tinymist_tool = resolve_tool(ToolKind::Tinymist, &settings.tinymist);
        context.egui_ctx.options_mut(|options| {
            options.zoom_with_keyboard = false;
            options.sync_window_theme = true;
            options.fallback_theme = egui::Theme::Dark;
        });
        configure_theme_styles(&context.egui_ctx);
        context
            .egui_ctx
            .set_theme(settings.interface_theme.egui_preference());
        let preview_dark =
            settings.document_theme.resolve(context.egui_ctx.theme()) == egui::Theme::Dark;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let (web_link_sender, web_link_receiver) = mpsc::channel();

        let mut app = Self {
            source: DEFAULT_SOURCE.to_owned(),
            saved_source: DEFAULT_SOURCE.to_owned(),
            path: None,
            document_epoch: 0,
            revision: 0,
            disk_fingerprint: None,
            document_kind: DocumentKind::Typst,
            highlighter: SyntaxHighlighter::default(),
            generic_highlighter: GenericSyntaxHighlighter::default(),
            compiler: Compiler::new(context.egui_ctx.clone()),
            asset_loader: AssetLoader::new(context.egui_ctx.clone()),
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
            view_mode: ViewMode::Split,
            filesystem_visible: true,
            problems_visible: false,
            settings_visible: false,
            settings: settings.clone(),
            pending_settings: None,
            applied_preview_preference: settings.preview_preference,
            applied_typst_preference: settings.typst.clone(),
            applied_tinymist_preference: settings.tinymist.clone(),
            tool_refresh_requested: false,
            typst_tool,
            tinymist_tool,
            workspace_root,
            workspace: None,
            workspace_error: None,
            next_workspace_refresh: Instant::now(),
            find_visible: false,
            replace_visible: false,
            find_query: String::new(),
            replacement: String::new(),
            search: SearchState::default(),
            focus_find: false,
            pending_editor_selection: None,
            editor_attention: None,
            reset_editor_history: true,
            editor_undo: Vec::new(),
            editor_redo: Vec::new(),
            autosave_deadline: None,
            diagnostic_tooltip: None,
            app_popup: None,
            app_popup_had_focus: false,
            pending_app_popup_action: None,
            app_modal: None,
            pending_document_action: None,
            post_save_action: None,
            rename_dialog: None,
            pending_export: None,
            pending_export_dialog: None,
            pending_tool_picker: None,
            pending_document_dialog: None,
            notice: None,
            last_title: String::new(),
            allow_close: false,
            tinymist: TinymistSidecar::new(context.egui_ctx.clone()),
            tinymist_generation: None,
            tinymist_uri: None,
            tinymist_url: None,
            tinymist_lsp_ready: false,
            tinymist_state: ServiceState::Starting("Launching Tinymist LSP".to_owned()),
            webview_state: ServiceState::Starting(
                "Waiting for Tinymist's preview server".to_owned(),
            ),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview_url: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            web_link_sender,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            web_link_receiver,
        };

        let remembered = initial_path
            .as_deref()
            .and_then(|path| {
                path.is_dir()
                    .then(|| remembered_document(&app.settings, path))
            })
            .flatten()
            .or_else(|| {
                initial_path
                    .is_none()
                    .then(|| remembered_document(&app.settings, &app.workspace_root))
                    .flatten()
            });
        if let Some(path) = initial_path.filter(|path| path.is_file()).or(remembered) {
            if !app.load_path(path) {
                app.reset_document_services();
                app.schedule_compile_now();
            }
        } else {
            app.reset_document_services();
        }
        app
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
        if self.document_kind.is_typst() {
            self.compile_deadline = Some(Instant::now() + COMPILE_DEBOUNCE);
            self.status = PreviewStatus::Waiting;
            self.sync_tinymist_change();
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
        if !self.document_kind.is_typst() {
            return;
        }
        let source_dir = self
            .current_directory()
            .unwrap_or_else(|| PathBuf::from("."));
        let request = CompileRequest {
            revision: self.revision,
            source: self.source.clone(),
            project_root: self.project_root(),
            source_dir,
            display_name: self.document_name(),
            typst_executable: self.typst_tool.program.clone(),
        };

        match self.compiler.request(request) {
            Ok(()) => self.status = PreviewStatus::Compiling,
            Err(error) => self.set_compile_error(error),
        }
    }

    fn tick_compile(&mut self, context: &egui::Context) {
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
            if !self.document_kind.is_typst() || result.revision != self.revision {
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
        let display_name = self.document_name();
        self.diagnostics = parse_typst_short_output(&raw, Some(Path::new(&display_name)));
        self.raw_diagnostics = raw;
    }

    fn schedule_compile_now(&mut self) {
        if !self.document_kind.is_typst() {
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
        self.pending_export = None;
        self.pending_export_dialog = None;
    }

    fn handle_shortcuts(&mut self, context: &egui::Context) {
        let save = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::S);
        let save_as = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::S);
        let open = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::O);
        let open_folder =
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::O);
        let new = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::N);
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

        if context.input_mut(|input| input.consume_shortcut(&save_as)) {
            self.save_as();
        } else if context.input_mut(|input| input.consume_shortcut(&save)) {
            self.save_document();
        }
        if context.input_mut(|input| input.consume_shortcut(&open)) {
            self.open_dialog();
        }
        if context.input_mut(|input| input.consume_shortcut(&open_folder)) {
            self.open_folder_dialog();
        }
        if context.input_mut(|input| input.consume_shortcut(&new)) {
            self.new_document();
        }
        if context.input_mut(|input| input.consume_shortcut(&refresh)) {
            self.request_compile();
            self.refresh_workspace();
        }
        if context.input_mut(|input| input.consume_shortcut(&export)) {
            self.export_pdf();
        }
        if context.input_mut(|input| input.consume_shortcut(&find)) {
            self.open_find(false);
        }
        if context.input_mut(|input| input.consume_shortcut(&settings)) {
            self.settings_visible = !self.settings_visible;
        }
        let source_focused = context.memory(|memory| memory.focused()) == Some(source_editor_id());
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
                self.requested_zoom =
                    Some((self.zoom * 1.15).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM));
                self.fit_width = false;
            }
            if context.input_mut(|input| input.consume_shortcut(&zoom_out)) {
                self.requested_zoom =
                    Some((self.zoom / 1.15).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM));
                self.fit_width = false;
            }
            if context.input_mut(|input| input.consume_shortcut(&zoom_reset)) {
                self.requested_zoom = Some(1.0);
                self.fit_width = false;
            }
        }
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

    fn queue_preview_preference(&mut self, preference: PreviewPreference, context: &egui::Context) {
        let mut settings = self
            .pending_settings
            .clone()
            .unwrap_or_else(|| self.settings.clone());
        settings.preview_preference = preference;
        self.queue_settings(settings, context);
    }

    fn sync_runtime_settings(&mut self, context: &egui::Context) {
        let next_preview_dark =
            self.settings.document_theme.resolve(context.theme()) == egui::Theme::Dark;
        let document_theme_changed = next_preview_dark != self.preview_dark;
        if document_theme_changed {
            self.preview_dark = next_preview_dark;
            self.rebuild_preview_textures(context);
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

        if document_theme_changed
            || preview_preference_changed
            || tinymist_program_changed
            || refresh_tools
        {
            self.restart_tinymist();
        }
    }

    fn handle_dropped_file(&mut self, context: &egui::Context) {
        let path = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .find(|path| !path.as_os_str().is_empty() && path.is_file())
        });
        if let Some(path) = path {
            self.request_document_replacement(
                DeferredDocumentAction::LoadPath(path),
                "opening another document",
            );
        }
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

    fn open_folder_dialog(&mut self) {
        self.request_document_replacement(
            DeferredDocumentAction::OpenFolderDialog,
            "opening another folder",
        );
    }

    fn start_open_folder_dialog(&mut self) {
        if self.pending_document_dialog.is_some() {
            return;
        }
        let mut dialog = AsyncFileDialog::new().set_title("Open project folder");
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.pending_document_dialog = Some(PendingDocumentDialog {
            target: DocumentDialogTarget::OpenFolder,
            document_epoch: self.document_epoch,
            revision: self.revision,
            future: Box::pin(dialog.pick_folder()),
        });
        self.autosave_deadline = None;
    }

    fn finish_open_folder_selection(&mut self, folder: PathBuf) {
        let root = canonical_or_absolute(&folder);
        if !root.is_dir() {
            self.show_file_error(format!("{} is not a folder", root.display()));
            return;
        }
        self.workspace_root = root.clone();
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

    fn open_dialog(&mut self) {
        self.request_document_replacement(
            DeferredDocumentAction::OpenFileDialog,
            "opening another document",
        );
    }

    fn start_open_dialog(&mut self) {
        if self.pending_document_dialog.is_some() {
            return;
        }
        let mut dialog = AsyncFileDialog::new()
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
        self.autosave_deadline = None;
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

    fn save_document(&mut self) -> bool {
        if !self.document_kind.is_editable() {
            return true;
        }
        if let Some(path) = self.path.clone() {
            self.save_to(path)
        } else {
            self.save_as()
        }
    }

    fn save_as(&mut self) -> bool {
        if !self.document_kind.is_editable() {
            return false;
        }
        if self.pending_document_dialog.is_some() {
            return false;
        }
        let typst = self.document_kind.is_typst();
        let mut dialog = AsyncFileDialog::new().set_file_name(self.document_name());
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
        self.autosave_deadline = None;
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
        match atomic_write(&path, self.source.as_bytes()) {
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
                }
                self.saved_source = self.source.clone();
                self.autosave_deadline = None;
                if let Some(path) = self.path.clone() {
                    self.remember_open_document(&path);
                }
                if let Some(action) = self.post_save_action.take() {
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
        let description = match fs::read(path) {
            Ok(contents) if fingerprint(&contents) == expected => return true,
            Ok(_) => format!(
                "{} changed in another application. Overwrite those newer changes?",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                format!(
                    "{} was deleted outside tiptoptyp. Create it again?",
                    path.display()
                )
            }
            Err(error) => {
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
        });
        false
    }

    fn export_pdf(&mut self) {
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
        let default_name = self
            .path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| format!("{}.pdf", stem.to_string_lossy()))
            .unwrap_or_else(|| "document.pdf".to_owned());
        let mut dialog = AsyncFileDialog::new()
            .add_filter("PDF documents", &["pdf"])
            .set_title("Export PDF")
            .set_file_name(default_name);
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        // NSSavePanel's synchronous `runModal` spins a nested AppKit loop from
        // inside winit's event callback. Dragging a folder across the panel can
        // then re-enter winit and trigger a non-unwinding panic. The async panel
        // uses a completion handler and is polled by normal egui frames instead.
        self.pending_export_dialog = Some(Box::pin(dialog.save_file()));
    }

    fn poll_export_dialog(&mut self, context: &egui::Context) {
        let Some(future) = self.pending_export_dialog.as_mut() else {
            return;
        };
        let waker = Waker::from(Arc::new(EguiFutureWake(context.clone())));
        let mut task_context = TaskContext::from_waker(&waker);
        let Poll::Ready(selection) = future.as_mut().poll(&mut task_context) else {
            context.request_repaint_after(Duration::from_millis(50));
            return;
        };
        self.pending_export_dialog = None;
        if let Some(file) = selection {
            self.finish_export_selection(file.path().to_path_buf());
        }
    }

    fn choose_tool_binary(&mut self, target: ToolPickerTarget) {
        if self.pending_tool_picker.is_some() {
            return;
        }
        let title = match target {
            ToolPickerTarget::Typst => "Choose Typst compiler",
            ToolPickerTarget::Tinymist => "Choose Tinymist language server",
        };
        let mut dialog = AsyncFileDialog::new().set_title(title);
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.pending_tool_picker = Some(PendingToolPicker {
            target,
            future: Box::pin(dialog.pick_file()),
        });
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
                if self.document_epoch != document_epoch || self.revision != revision {
                    self.request_document_replacement(
                        DeferredDocumentAction::LoadPath(path),
                        "opening the selected document",
                    );
                } else {
                    self.load_path(path);
                }
            }
            DocumentDialogTarget::OpenFolder => {
                if self.document_epoch != document_epoch || self.revision != revision {
                    self.request_document_replacement(
                        DeferredDocumentAction::OpenFolder(path),
                        "opening the selected folder",
                    );
                } else {
                    self.finish_open_folder_selection(path);
                }
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
            if let Err(error) = atomic_write(&path, pdf) {
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
            revision: self.revision,
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
            take_matching_export(&mut self.pending_export, self.revision),
            self.pdf.as_deref(),
        ) else {
            return;
        };
        if let Err(error) = atomic_write(&path, pdf) {
            self.show_file_error(error);
        } else {
            self.notice = Some(Notice {
                message: format!("Exported {}", path.display()),
                kind: NoticeKind::Success,
            });
        }
    }

    fn request_document_replacement(&mut self, action: DeferredDocumentAction, description: &str) {
        if self.is_dirty() {
            if self.app_modal.is_none() {
                self.app_modal = Some(AppModal::Unsaved {
                    message: format!(
                        "Save changes to {} before {description}?",
                        self.document_name()
                    ),
                    action,
                });
            }
        } else {
            self.pending_document_action = Some(action);
        }
    }

    fn execute_pending_document_action(&mut self, context: &egui::Context) {
        let Some(action) = self.pending_document_action.take() else {
            return;
        };
        match action {
            DeferredDocumentAction::New => self.reset_untitled_document(),
            DeferredDocumentAction::OpenFileDialog => self.start_open_dialog(),
            DeferredDocumentAction::OpenFolderDialog => self.start_open_folder_dialog(),
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
            DeferredDocumentAction::SaveThen(action) => {
                self.post_save_action = Some(*action);
                if self.save_document()
                    && let Some(action) = self.post_save_action.take()
                {
                    self.pending_document_action = Some(action);
                }
            }
            DeferredDocumentAction::ForceSave(path) => {
                self.save_to_with_intent(path, SaveIntent::ExplicitConfirmed);
            }
        }
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
    }

    fn reset_document_services(&mut self) {
        let root = self.project_root();
        match WorkspaceTree::new(&root) {
            Ok(workspace) => {
                self.workspace = Some(workspace);
                self.workspace_error = None;
            }
            Err(error) => {
                self.workspace = None;
                self.workspace_error = Some(format!("Could not scan {}: {error}", root.display()));
            }
        }
        self.next_workspace_refresh = Instant::now() + WORKSPACE_REFRESH_INTERVAL;
        self.restart_tinymist();
    }

    fn refresh_workspace(&mut self) {
        if let Some(workspace) = &mut self.workspace {
            match workspace.refresh() {
                Ok(_) => self.workspace_error = None,
                Err(error) => self.workspace_error = Some(error.to_string()),
            }
        }
        self.next_workspace_refresh = Instant::now() + WORKSPACE_REFRESH_INTERVAL;
    }

    fn tick_workspace(&mut self, context: &egui::Context) {
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
        self.path
            .clone()
            .unwrap_or_else(|| self.project_root().join(".mytypst-untitled.typ"))
    }

    fn restart_tinymist(&mut self) {
        if let Some(generation) = self.tinymist_generation.take() {
            if let Some(uri) = self.tinymist_uri.take() {
                let _ = self.tinymist.did_close(generation, uri);
            }
            let _ = self.tinymist.stop_workspace(generation);
        }
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
        } else if cfg!(any(target_os = "macos", target_os = "windows")) {
            ServiceState::Starting("Waiting for Tinymist's preview server".to_owned())
        } else {
            ServiceState::Unsupported(
                "Embedded Tinymist preview is currently available on macOS and Windows".to_owned(),
            )
        };
        let mut config = TinymistConfig::new(self.project_root())
            .with_executable(self.tinymist_tool.program.clone());
        config.start_preview = self.settings.preview_preference == PreviewPreference::Interactive
            && cfg!(any(target_os = "macos", target_os = "windows"));
        config.preview.invert_colors = if self.preview_dark {
            InvertColors::Always
        } else {
            InvertColors::Never
        };
        match self.tinymist.start_workspace(config) {
            Ok(generation) => {
                let path = self.tinymist_document_path();
                match TextDocument::from_path(
                    &path,
                    revision_as_i32(self.revision),
                    self.source.clone(),
                ) {
                    Ok(document) => {
                        self.tinymist_uri = Some(document.uri.clone());
                        self.tinymist_generation = Some(generation);
                        if let Err(error) = self.tinymist.did_open(generation, document) {
                            self.tinymist_state = ServiceState::Failed(error.to_string());
                        }
                    }
                    Err(error) => {
                        self.tinymist_state = ServiceState::Failed(error.to_string());
                    }
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
        let (Some(generation), Some(uri)) = (self.tinymist_generation, self.tinymist_uri.clone())
        else {
            return;
        };
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
                TinymistEvent::Initialized { .. } => {
                    self.tinymist_lsp_ready = true;
                    self.tinymist_state = if self.settings.preview_preference
                        == PreviewPreference::Interactive
                        && cfg!(any(target_os = "macos", target_os = "windows"))
                    {
                        ServiceState::Starting("Starting Tinymist preview server".to_owned())
                    } else {
                        ServiceState::Ready("Tinymist LSP is ready".to_owned())
                    };
                }
                TinymistEvent::PreviewReady { url, .. } => {
                    self.tinymist_state =
                        ServiceState::Ready("LSP and preview server are ready".to_owned());
                    if self.settings.preview_preference == PreviewPreference::Interactive
                        && cfg!(any(target_os = "macos", target_os = "windows"))
                    {
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
            let range = lsp_range_to_char_range(&self.source, selection);
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
        let (line, character) = source_position_at_char(&self.source, char_index);
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

    fn receive_tinymist_diagnostics(
        &mut self,
        uri: &str,
        version: Option<i32>,
        diagnostics: Vec<TinymistDiagnostic>,
    ) {
        if version.is_some_and(|version| version != revision_as_i32(self.revision)) {
            return;
        }
        let source = if self.tinymist_uri.as_deref() == Some(uri) {
            DiagnosticSource::Main
        } else {
            url::Url::parse(uri)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .map_or(DiagnosticSource::Global, DiagnosticSource::File)
        };
        self.tinymist_diagnostics = diagnostics
            .into_iter()
            .map(|diagnostic| tinymist_diagnostic(diagnostic, source.clone()))
            .collect();
    }

    fn interactive_preview_active(&self) -> bool {
        self.document_kind.is_typst()
            && self.settings.preview_preference == PreviewPreference::Interactive
            && self.tinymist_url.is_some()
            && self.webview_state.is_ready()
            && cfg!(any(target_os = "macos", target_os = "windows"))
    }

    fn should_attempt_interactive_preview(&self) -> bool {
        self.document_kind.is_typst()
            && self.settings.preview_preference == PreviewPreference::Interactive
            && self.tinymist_url.is_some()
            && !matches!(
                self.webview_state,
                ServiceState::Failed(_) | ServiceState::Unsupported(_)
            )
            && cfg!(any(target_os = "macos", target_os = "windows"))
    }

    fn preview_fallback_reason(&self) -> Option<String> {
        if !self.document_kind.is_typst() {
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
                DocumentKind::Pdf => "Native PDF",
                DocumentKind::Image => "Native image",
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

    fn show_toolbar(&mut self, ui: &mut egui::Ui, _frame: &eframe::Frame) {
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
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);

            #[cfg(target_os = "macos")]
            {
                use raw_window_handle::HasWindowHandle as _;

                let traffic_lights_width = _frame
                    .window_handle()
                    .ok()
                    .and_then(|handle| {
                        eframe::WindowChromeMetrics::from_window_handle(&handle.as_raw())
                    })
                    .map(|metrics| metrics.traffic_lights_size.x / ui.ctx().zoom_factor().max(0.1))
                    .unwrap_or(64.0);
                ui.add_space(traffic_lights_width + 4.0);
            }

            let toolbar_width = ui.available_width();
            let compact = toolbar_width < 620.0;
            if compact {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.spacing_mut().button_padding.x = 3.0;
            }

            let title = format!(
                "{}{}",
                self.document_name(),
                if self.is_dirty() { "*" } else { "" }
            );
            let title_width = if compact {
                (toolbar_width * 0.14).clamp(28.0, 76.0)
            } else {
                (toolbar_width * 0.16).clamp(90.0, 190.0)
            };
            let title_response = ui.add_sized(
                [title_width, 20.0],
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
            if title_response.double_clicked() {
                if let Some(path) = self.path.clone() {
                    self.begin_rename(path);
                } else {
                    self.save_as();
                }
            }
            ui.separator();

            self.show_file_menu(ui);
            self.show_edit_menu(ui);
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

            ui.separator();
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
            if native_hover_text(ui.button("Find"), "Find").clicked() {
                self.open_find(false);
            }

            ui.separator();
            let explorer_label = if compact { "Files" } else { "Explorer" };
            if native_hover_text(
                ui.selectable_label(self.filesystem_visible, explorer_label),
                "Toggle the file explorer",
            )
            .clicked()
            {
                self.filesystem_visible = !self.filesystem_visible;
            }
            native_hover_text(
                ui.selectable_value(
                    &mut self.view_mode,
                    ViewMode::Code,
                    if compact { "C" } else { "Code" },
                ),
                "Code",
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
                    ViewMode::Preview,
                    if compact { "P" } else { "Preview" },
                ),
                "Preview",
            );
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
        context.memory_mut(|memory| memory.request_focus(source_editor_id()));
        if changed {
            self.search.clear();
            self.mark_edited();
        }
    }

    fn editor_snapshot(&self, context: &egui::Context) -> EditorSnapshot {
        let cursor = egui::text_edit::TextEditState::load(context, source_editor_id())
            .and_then(|state| state.cursor.char_range())
            .unwrap_or_else(|| CCursorRange::one(CCursor::new(self.source.chars().count())));
        EditorSnapshot {
            source: self.source.clone(),
            cursor: clamp_cursor_range(cursor, self.source.chars().count()),
        }
    }

    fn store_editor_cursor(&self, context: &egui::Context, cursor: CCursorRange) {
        let mut state =
            egui::text_edit::TextEditState::load(context, source_editor_id()).unwrap_or_default();
        state.clear_undoer();
        state.cursor.set_char_range(Some(clamp_cursor_range(
            cursor,
            self.source.chars().count(),
        )));
        state.store(context, source_editor_id());
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
        let state = egui::text_edit::TextEditState::load(context, source_editor_id())?;
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
        if let Some(mut state) = egui::text_edit::TextEditState::load(context, source_editor_id()) {
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(range.start))));
            state.store(context, source_editor_id());
        }
        context.memory_mut(|memory| memory.request_focus(source_editor_id()));
        self.search.clear();
        self.mark_edited();
    }

    fn request_editor_paste(&mut self, context: &egui::Context) {
        if self.document_kind.is_editable() {
            if self.document_kind.is_typst() && !self.view_mode.shows_code() {
                self.view_mode = ViewMode::Split;
            }
            context.memory_mut(|memory| memory.request_focus(source_editor_id()));
            context.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
    }

    fn select_all_editor(&self, context: &egui::Context) {
        if let Some(mut state) = egui::text_edit::TextEditState::load(context, source_editor_id()) {
            state.cursor.set_char_range(Some(CCursorRange::two(
                CCursor::new(0),
                CCursor::new(self.source.chars().count()),
            )));
            state.store(context, source_editor_id());
            context.memory_mut(|memory| memory.request_focus(source_editor_id()));
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
        let (formatted, mapped_cursor) =
            match apply_lsp_text_edits_and_map_cursor(&self.source, &edits, cursor) {
                Ok(result) => result,
                Err(error) => {
                    self.notice = Some(Notice {
                        message: format!("Tinymist returned invalid formatting edits: {error}"),
                        kind: NoticeKind::Error,
                    });
                    return;
                }
            };
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
    }

    fn show_rename_dialog(&mut self, context: &egui::Context) {
        if self.rename_dialog.is_none() || self.app_modal.is_some() {
            return;
        }
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let native_theme = match theme {
            egui::Theme::Dark => egui::SystemTheme::Dark,
            egui::Theme::Light => egui::SystemTheme::Light,
        };
        let Some(dialog) = &mut self.rename_dialog else {
            return;
        };
        let mut submit = false;
        let mut cancel = false;
        context.show_viewport_immediate(
            egui::ViewportId::from_hash_of("tiptoptyp-rename-overlay"),
            egui::ViewportBuilder::default()
                .with_title("Rename")
                .with_position(window_rect.min)
                .with_inner_size(window_rect.size())
                .with_min_inner_size(window_rect.size())
                .with_max_inner_size(window_rect.size())
                .with_resizable(false)
                .with_transparent(true)
                .with_decorations(false)
                .with_active(true)
                .with_taskbar(false)
                .with_close_button(false)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_has_shadow(false)
                .with_always_on_top(),
            |ui, _class| {
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                cancel |= ui.ctx().input(|input| {
                    input.viewport().close_requested() || input.key_pressed(egui::Key::Escape)
                });
                egui::Area::new(egui::Id::new("rename-document-dialog"))
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        popup_card_frame(&style)
                            .outer_margin(egui::Margin::same(8))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_min_width(340.0);
                                ui.label(RichText::new("Rename").strong());
                                let response = ui.add_sized(
                                    [320.0, 24.0],
                                    egui::TextEdit::singleline(&mut dialog.name)
                                        .id_salt("rename-name"),
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
            },
        );
        if cancel {
            self.rename_dialog = None;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
        } else if submit {
            let dialog = self.rename_dialog.take().expect("rename dialog exists");
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.commit_rename(dialog.path, dialog.name);
        }
    }

    fn show_app_modal_window(&mut self, context: &egui::Context) {
        let Some(modal) = self.app_modal.clone() else {
            return;
        };
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let native_theme = match theme {
            egui::Theme::Dark => egui::SystemTheme::Dark,
            egui::Theme::Light => egui::SystemTheme::Light,
        };
        let card_width = (window_rect.width() - 20.0).clamp(1.0, 460.0);
        let mut choice = None;

        context.show_viewport_immediate(
            egui::ViewportId::from_hash_of("tiptoptyp-modal-overlay"),
            egui::ViewportBuilder::default()
                .with_title("tiptoptyp")
                .with_position(window_rect.min)
                .with_inner_size(window_rect.size())
                .with_min_inner_size(window_rect.size())
                .with_max_inner_size(window_rect.size())
                .with_resizable(false)
                .with_transparent(true)
                .with_decorations(false)
                .with_active(true)
                .with_taskbar(false)
                .with_close_button(false)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_has_shadow(false)
                .with_always_on_top(),
            |ui, _class| {
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                if ui.ctx().input(|input| {
                    input.viewport().close_requested() || input.key_pressed(egui::Key::Escape)
                }) {
                    choice = Some(AppModalChoice::Cancel);
                }
                egui::Area::new(egui::Id::new("app-modal-card"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        popup_card_frame(&style)
                            .outer_margin(egui::Margin::same(8))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
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
                                ui.label(message);
                                ui.add_space(6.0);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    match &modal {
                                        AppModal::Alert { .. } => {
                                            if ui.button("OK").clicked() {
                                                choice = Some(AppModalChoice::Primary);
                                            }
                                        }
                                        AppModal::Unsaved { .. } => {
                                            if ui.button("Save").clicked() {
                                                choice = Some(AppModalChoice::Primary);
                                            }
                                            if ui.button("Don't Save").clicked() {
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
                                    }
                                });
                            });
                    });
            },
        );

        let Some(choice) = choice else {
            return;
        };
        self.app_modal = None;
        context.send_viewport_cmd(egui::ViewportCommand::Focus);
        match (modal, choice) {
            (AppModal::Alert { .. }, _) => {}
            (AppModal::Unsaved { action, .. }, AppModalChoice::Primary) => {
                self.pending_document_action =
                    Some(DeferredDocumentAction::SaveThen(Box::new(action)));
            }
            (AppModal::Unsaved { action, .. }, AppModalChoice::Secondary) => {
                self.post_save_action = None;
                self.pending_document_action = Some(action);
            }
            (AppModal::Unsaved { .. }, AppModalChoice::Cancel) => {
                self.post_save_action = None;
            }
            (AppModal::Overwrite { path, .. }, AppModalChoice::Primary) => {
                self.pending_document_action = Some(DeferredDocumentAction::ForceSave(path));
            }
            (AppModal::Overwrite { .. }, _) => {
                self.post_save_action = None;
                self.schedule_autosave_if_needed();
            }
        }
        context.request_repaint();
    }

    fn commit_rename(&mut self, old_path: PathBuf, name: String) {
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
        self.notice = Some(Notice {
            message: format!("Renamed to {}", new_path.display()),
            kind: NoticeKind::Success,
        });
    }

    fn show_settings_window(&mut self, context: &egui::Context) {
        if !self.settings_visible {
            return;
        }
        let theme = self
            .settings
            .interface_theme
            .resolve(context.system_theme(), egui::Theme::Dark);
        let style = context.style_of(theme);
        let native_theme = match theme {
            egui::Theme::Dark => egui::SystemTheme::Dark,
            egui::Theme::Light => egui::SystemTheme::Light,
        };
        let mut close_requested = false;
        context.show_viewport_immediate(
            egui::ViewportId::from_hash_of("tiptoptyp-settings"),
            egui::ViewportBuilder::default()
                .with_title("tiptoptyp Settings")
                .with_inner_size([SETTINGS_PANEL_WIDTH, 560.0])
                .with_min_inner_size([360.0, 360.0]),
            |ui, _class| {
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                close_requested |= ui.ctx().input(|input| input.viewport().close_requested());
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
                egui::Frame::NONE
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| self.show_settings(ui));
            },
        );
        if close_requested {
            self.settings_visible = false;
        }
    }

    fn show_diagnostic_tooltip_window(&self, context: &egui::Context) {
        let (anchor, detail, severity) = if let Some(tooltip) = self.diagnostic_tooltip.clone() {
            (tooltip.anchor, tooltip.detail, Some(tooltip.severity))
        } else if let Some(tooltip) =
            context.data(|data| data.get_temp::<HoverTooltipOverlay>(native_hover_tooltip_id()))
        {
            (tooltip.anchor, tooltip.detail, None)
        } else {
            return;
        };
        let root_ready = context.input(|input| {
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
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };

        const CARD_MARGIN: f32 = 8.0;
        let theme = context.theme();
        let style = context.style_of(theme);
        let body_font = egui::TextStyle::Body.resolve(&style);
        let desired_card_width = if severity.is_some() {
            DIAGNOSTIC_TOOLTIP_WIDTH
        } else {
            let natural_width = context.fonts_mut(|fonts| {
                fonts
                    .layout(
                        detail.clone(),
                        body_font.clone(),
                        style.visuals.text_color(),
                        f32::INFINITY,
                    )
                    .size()
                    .x
            });
            (natural_width + 18.0).clamp(96.0, 320.0)
        };
        let available_width = (window_rect.width() - 8.0).max(1.0);
        let width = (desired_card_width + CARD_MARGIN * 2.0).min(available_width);
        let card_width = (width - CARD_MARGIN * 2.0).max(1.0);
        let body_height = context.fonts_mut(|fonts| {
            fonts
                .layout(
                    detail.clone(),
                    body_font,
                    style.visuals.text_color(),
                    (card_width - 18.0).max(1.0),
                )
                .size()
                .y
        });
        let available_height = (window_rect.height() - 8.0).max(1.0);
        let title_height = if severity.is_some() { 48.0 } else { 26.0 };
        let height = (title_height + body_height + CARD_MARGIN * 2.0)
            .clamp(68.0, 256.0)
            .min(available_height);
        let desired = window_rect.min + anchor.to_vec2();
        let position = Pos2::new(
            desired
                .x
                .clamp(window_rect.left() + 4.0, window_rect.right() - width - 4.0),
            desired
                .y
                .clamp(window_rect.top() + 4.0, window_rect.bottom() - height - 4.0),
        );
        let native_theme = match theme {
            egui::Theme::Dark => egui::SystemTheme::Dark,
            egui::Theme::Light => egui::SystemTheme::Light,
        };

        context.show_viewport_immediate(
            egui::ViewportId::from_hash_of("diagnostic-tooltip-overlay"),
            egui::ViewportBuilder::default()
                .with_title("Diagnostic")
                .with_position(position)
                .with_inner_size([width, height])
                .with_min_inner_size([width, height])
                .with_max_inner_size([width, height])
                .with_resizable(false)
                .with_transparent(true)
                .with_decorations(false)
                .with_active(false)
                .with_taskbar(false)
                .with_has_shadow(false)
                .with_always_on_top()
                .with_mouse_passthrough(true),
            |ui, _class| {
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                popup_card_frame(&style)
                    .outer_margin(egui::Margin::same(CARD_MARGIN as i8))
                    .inner_margin(egui::Margin::same(9))
                    .show(ui, |ui| {
                        if let Some(severity) = severity {
                            let color = diagnostic_color(severity, ui.visuals().dark_mode);
                            ui.label(RichText::new(severity.label()).strong().color(color));
                        }
                        ui.label(&detail);
                    });
            },
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
        let native_theme = match theme {
            egui::Theme::Dark => egui::SystemTheme::Dark,
            egui::Theme::Light => egui::SystemTheme::Light,
        };
        let (anchor, estimated_size) = match &popup {
            AppPopup::File { anchor } => (*anchor, egui::vec2(260.0, 220.0)),
            AppPopup::Edit { anchor } => (*anchor, egui::vec2(280.0, 320.0)),
            AppPopup::Workspace { anchor, .. } => (*anchor, egui::vec2(220.0, 150.0)),
            AppPopup::Editor { anchor } => (*anchor, egui::vec2(220.0, 240.0)),
        };
        let anchor = clamp_popup_anchor(anchor, estimated_size, window_rect.size());
        let (can_undo, can_redo) = self.editor_history_availability(context);
        let has_selection = self.selected_editor_chars(context).is_some();
        let can_format = self.document_kind.is_typst();
        let mut close = false;
        let mut action = None;
        let mut had_focus = self.app_popup_had_focus;

        context.show_viewport_immediate(
            egui::ViewportId::from_hash_of("tiptoptyp-popup-overlay"),
            egui::ViewportBuilder::default()
                .with_title("tiptoptyp menu")
                .with_position(window_rect.min + anchor.to_vec2())
                .with_inner_size(estimated_size)
                .with_min_inner_size(estimated_size)
                .with_max_inner_size(estimated_size)
                .with_resizable(false)
                .with_transparent(true)
                .with_decorations(false)
                .with_active(true)
                .with_taskbar(false)
                .with_close_button(false)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_has_shadow(false)
                .with_always_on_top(),
            |ui, _class| {
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

                egui::Area::new(egui::Id::new("app-popup-card"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(Pos2::ZERO)
                    .show(ui.ctx(), |ui| {
                        popup_card_frame(&style)
                            .outer_margin(egui::Margin::same(7))
                            .show(ui, |ui| match &popup {
                                AppPopup::File { .. } => {
                                    ui.set_min_width(240.0);
                                    show_file_popup_ui(ui, &mut action);
                                }
                                AppPopup::Edit { .. } => {
                                    ui.set_min_width(260.0);
                                    show_edit_popup_ui(
                                        ui,
                                        can_undo,
                                        can_redo,
                                        can_format,
                                        &mut action,
                                    );
                                }
                                AppPopup::Workspace { path, is_file, .. } => {
                                    ui.set_min_width(200.0);
                                    show_workspace_popup_ui(ui, path, *is_file, &mut action);
                                }
                                AppPopup::Editor { .. } => {
                                    ui.set_min_width(200.0);
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
                            });
                    });
            },
        );
        self.app_popup_had_focus = had_focus;

        if close || action.is_some() {
            self.app_popup = None;
            self.app_popup_had_focus = false;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if let Some(action) = action {
            self.pending_app_popup_action = Some(action);
            context.request_repaint();
        }
    }

    fn execute_pending_app_popup_action(&mut self, context: &egui::Context) {
        let Some(action) = self.pending_app_popup_action.take() else {
            return;
        };
        match action {
            AppPopupAction::File(action) => match action {
                FileMenuAction::New => self.new_document(),
                FileMenuAction::Open => self.open_dialog(),
                FileMenuAction::OpenFolder => self.open_folder_dialog(),
                FileMenuAction::Save => {
                    self.save_document();
                }
                FileMenuAction::SaveAs => {
                    self.save_as();
                }
                FileMenuAction::ExportPdf => self.export_pdf(),
            },
            AppPopupAction::Editor(action) => match action {
                EditorMenuAction::Undo => self.undo_editor(context, false),
                EditorMenuAction::Redo => self.undo_editor(context, true),
                EditorMenuAction::Cut => self.cut_editor_selection(context),
                EditorMenuAction::Copy => self.copy_editor_selection(context),
                EditorMenuAction::Paste => self.request_editor_paste(context),
                EditorMenuAction::SelectAll => self.select_all_editor(context),
                EditorMenuAction::Format => self.request_format_document(),
            },
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

    fn show_settings(&mut self, ui: &mut egui::Ui) {
        ui.set_max_width(SETTINGS_PANEL_WIDTH.min(ui.available_width()));
        ui.heading("Settings");
        ui.separator();

        let mut edited = self
            .pending_settings
            .clone()
            .unwrap_or_else(|| self.settings.clone());
        egui::ScrollArea::vertical()
            .id_salt("settings-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading("Appearance");
                let system_theme = ui.ctx().system_theme();
                let effective_theme = ui.ctx().theme();
                let selected_effective_theme = edited
                    .interface_theme
                    .resolve(system_theme, egui::Theme::Dark);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Interface").strong());
                    for theme in InterfaceTheme::ALL {
                        ui.selectable_value(&mut edited.interface_theme, theme, theme.label());
                    }
                    ui.separator();
                    settings_inline_value(ui, "System", theme_label(system_theme));
                    ui.separator();
                    settings_inline_value(
                        ui,
                        "Effective",
                        theme_label(Some(selected_effective_theme)),
                    );
                    if selected_effective_theme != effective_theme {
                        ui.label(RichText::new("applies next frame").small().weak());
                    }
                });
                if let Some(reason) = edited.interface_theme.fallback_reason(system_theme) {
                    fallback_notice(ui, "THEME FALLBACK", reason);
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

                ui.add_space(4.0);
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
                    ui.label("Jump from source to preview");
                    egui::ComboBox::from_id_salt("source-preview-trigger")
                        .selected_text(edited.source_preview_trigger.label())
                        .show_ui(ui, |ui| {
                            for trigger in SourcePreviewTrigger::ALL {
                                ui.selectable_value(
                                    &mut edited.source_preview_trigger,
                                    trigger,
                                    trigger.label(),
                                );
                            }
                        });
                    ui.label(
                        RichText::new(edited.source_preview_trigger.description())
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                });

                ui.add_space(4.0);
                ui.separator();
                ui.heading("Tool binaries");
                ui.label(
                    RichText::new(
                        "Packaged builds use pinned sidecars. A custom path overrides one tool without changing the other.",
                    )
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
                if tool_preference_editor(ui, "Typst compiler", &mut edited.typst) {
                    self.choose_tool_binary(ToolPickerTarget::Typst);
                }
                let staged_typst = if edited.typst == self.settings.typst {
                    self.typst_tool.clone()
                } else {
                    resolve_tool(ToolKind::Typst, &edited.typst)
                };
                show_tool_resolution(ui, &staged_typst);
                ui.add_space(5.0);
                if tool_preference_editor(ui, "Tinymist language server", &mut edited.tinymist) {
                    self.choose_tool_binary(ToolPickerTarget::Tinymist);
                }
                let staged_tinymist = if edited.tinymist == self.settings.tinymist {
                    self.tinymist_tool.clone()
                } else {
                    resolve_tool(ToolKind::Tinymist, &edited.tinymist)
                };
                show_tool_resolution(ui, &staged_tinymist);
                if ui.button("Refresh binary status").clicked() {
                    self.tool_refresh_requested = true;
                    ui.ctx().request_repaint();
                }

                ui.add_space(4.0);
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
                    settings_inline_value(ui, "Effective", self.preview_backend_label());
                });
                if let Some(reason) = self.preview_fallback_reason() {
                    fallback_notice(ui, "PREVIEW FALLBACK ACTIVE", &reason);
                }
                if self.settings.preview_preference == PreviewPreference::Interactive
                    && !self.interactive_preview_active()
                    && ui.button("Retry Tinymist").clicked()
                {
                    self.restart_tinymist();
                }

                ui.add_space(4.0);
                ui.separator();
                ui.heading("Toolchain status");
                ui.horizontal_wrapped(|ui| {
                    let syntax_color = success_color(ui.visuals().dark_mode);
                    show_tool_status_chip(ui, "Typst", &self.typst_tool);
                    show_tool_status_chip(ui, "Tinymist", &self.tinymist_tool);
                    show_service_status_chip(ui, "LSP", &self.tinymist_state);
                    show_service_status_chip(ui, "Vector", &self.webview_state);
                    show_service_status_chip(ui, "Watcher", &self.compiler_service_state());
                    show_service_status_chip(ui, "PDF", &self.rasterizer_service_state());
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
                    &self.project_root().display().to_string(),
                );
            });

        self.queue_settings(edited, ui.ctx());
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
        let root = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().display().to_string())
            .unwrap_or_else(|| self.project_root().display().to_string());
        let generation = self.workspace.as_ref().map(WorkspaceTree::generation);
        show_panel_header(ui, "workspace-header", |ui| {
            let refresh_width = 24.0;
            let path_width =
                (ui.available_width() - refresh_width - ui.spacing().item_spacing.x).max(1.0);
            let max_chars = approximate_char_capacity(path_width, 12.0);
            let response = ui.add_sized(
                [path_width, 20.0],
                egui::Label::new(
                    RichText::new(tail_elide(&root, max_chars))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                )
                .truncate(),
            );
            let hover = generation.map_or_else(
                || root.clone(),
                |generation| format!("{root}\nFilesystem snapshot generation {generation}"),
            );
            native_hover_text(response, hover);
            if icon_button(ui, UiIcon::Refresh, "Refresh filesystem").clicked() {
                self.refresh_workspace();
            }
        });

        // A tree row can be much wider than the pane. Keep the body width in a
        // clipped child UI so it becomes scrollable content instead of feeding
        // back into `PanelState` and growing the resizable explorer each frame.
        let mut content_ui = clipped_panel_content_ui(ui, "workspace-clipped-content");
        let ui = &mut content_ui;

        let snapshot = self.workspace.as_ref().map(|tree| tree.snapshot().clone());
        let active = snapshot.as_ref().and_then(|snapshot| {
            self.path.as_ref().and_then(|path| {
                path.strip_prefix(&snapshot.root)
                    .ok()
                    .and_then(|relative| snapshot.find(relative))
                    .map(|node| node.path.clone())
            })
        });
        let mut open_path = None;
        let mut popup_request = None;
        egui::ScrollArea::both()
            .id_salt("workspace-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(snapshot) = &snapshot {
                    let tree = TreeView::new(ui.id().with("workspace-tree"))
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
                    let (_, actions) = tree.show(ui, |builder| {
                        add_workspace_nodes(builder, &snapshot.nodes, active.as_deref());
                    });
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
            });

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
        let matches = if find(&self.source, &self.find_query).is_some() {
            find_all(&self.source, &self.find_query).len()
        } else {
            0
        };

        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.find_query)
                    .id_salt("find-query")
                    .hint_text("Find")
                    .desired_width(180.0),
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
            ui.label(RichText::new(format!("{matches} matches")).small().weak());
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
                        .desired_width(180.0),
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
        if self.reset_editor_history {
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), source_editor_id())
                .unwrap_or_default();
            state.clear_undoer();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            state.store(ui.ctx(), source_editor_id());
            self.reset_editor_history = false;
        }
        if self.find_visible {
            self.show_find_bar(ui);
            ui.separator();
        }

        let line_diagnostics = self.line_diagnostics();
        let available = ui.available_size();
        let line_count = logical_line_count(&self.source);
        let content_height = line_count as f32 * 20.0 + 8.0;
        let needs_vertical_scroll = content_height > available.y;
        let editor_height = if needs_vertical_scroll {
            content_height
        } else {
            (available.y - 1.0).max(24.0)
        };
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
        let unwrapped_editor_width = available
            .x
            .max((longest_line as f32 * 8.5) + (longest_diagnostic as f32 * 7.2) + 100.0)
            .max(640.0);
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
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        } else {
            self.editor_attention = None;
        }
        let mut changed = false;
        let mut preview_jump_char = None;
        let mut popup_request = None;

        egui::ScrollArea::new([!line_wrap, true])
            .id_salt("source-editor-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let editor_width = if line_wrap {
                    ui.available_width().max(24.0)
                } else {
                    unwrapped_editor_width
                };
                let editor_base_slot = ui.painter().add(egui::Shape::Noop);
                let current_line_slot = ui.painter().add(egui::Shape::Noop);
                let background_slots = line_diagnostics
                    .iter()
                    .map(|_| ui.painter().add(egui::Shape::Noop))
                    .collect::<Vec<_>>();
                let mut layouter =
                    |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                        let mut job = if document_kind.is_typst() {
                            highlighter.highlight(buffer.as_str(), dark_mode)
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
                    .id(source_editor_id())
                    .code_editor()
                    .desired_width(editor_width)
                    .min_size(Vec2::new(editor_width, editor_height))
                    // The normal opaque TextEdit frame would cover the line
                    // decoration slots inserted immediately before it. Paint
                    // the same base explicitly, then keep the widget frame
                    // transparent so backgrounds remain behind glyphs.
                    .background_color(Color32::TRANSPARENT)
                    .margin(egui::Margin {
                        left: gutter_width,
                        right: 4,
                        top: 2,
                        bottom: 2,
                    })
                    .layouter(&mut layouter);
                let mut output = editor.show(ui);
                changed = output.response.changed();
                ui.painter().set(
                    editor_base_slot,
                    egui::Shape::rect_filled(
                        output.response.rect,
                        0.0,
                        ui.visuals().text_edit_bg_color(),
                    ),
                );
                let mut current_char = output
                    .state
                    .cursor
                    .char_range()
                    .map(|range| range.primary.index.0);

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
                    current_line_slot,
                );
                self.diagnostic_tooltip =
                    paint_line_diagnostics(ui, &output, &line_diagnostics, background_slots);
                if line_numbers {
                    paint_line_numbers(ui, &output);
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
            });

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

    fn line_diagnostics(&self) -> Vec<LineDiagnostic> {
        let mut by_line: BTreeMap<usize, Vec<&Diagnostic>> = BTreeMap::new();
        for diagnostic in self.diagnostics.iter().chain(&self.tinymist_diagnostics) {
            if diagnostic.is_for_main_file()
                && let Some(line) = diagnostic.line()
            {
                let entries = by_line.entry(line).or_default();
                if !entries.iter().any(|existing| {
                    existing.message == diagnostic.message
                        && existing.severity == diagnostic.severity
                }) {
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
                    .map(|diagnostic| {
                        format!(
                            "{}: {}",
                            diagnostic.severity.label(),
                            diagnostic.full_message()
                        )
                    })
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
        // This fixed-height strip contains controls only. Transient build,
        // stale, and fallback state belongs exclusively in the bottom bar so
        // it can never alter the preview viewport's origin or extent.
        show_panel_header(ui, "preview-header", |ui| {
            self.show_preview_header_controls(ui);
        });

        if self.should_attempt_interactive_preview() {
            let rect = ui.available_rect_before_wrap();
            if self.update_webview(ui.ctx(), frame, rect, ui.visuals().panel_fill, true) {
                let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                ui.painter().rect_filled(rect, 0.0, preview_background(ui));
            } else {
                self.hide_webview();
                self.show_native_preview(ui);
            }
        } else {
            self.hide_webview();
            self.show_native_preview(ui);
        }
    }

    fn show_preview_header_controls(&mut self, ui: &mut egui::Ui) {
        let header_width = ui.available_width();
        if self.document_kind.is_typst() {
            let (label, next, hover) = match self.settings.preview_preference {
                PreviewPreference::Interactive => (
                    "Interactive",
                    PreviewPreference::Native,
                    "Interactive vector preview · click for the native PDF viewer",
                ),
                PreviewPreference::Native => (
                    "Native",
                    PreviewPreference::Interactive,
                    "Native PDF viewer · click for the interactive vector preview",
                ),
            };
            if native_hover_text(ui.selectable_label(true, label), hover).clicked() {
                self.queue_preview_preference(next, ui.ctx());
            }
        } else {
            native_hover_text(
                ui.label(RichText::new("Native").strong()),
                "This file opens directly in the native viewer",
            );
        }

        if !self.interactive_preview_active() {
            let page_count = self.pages.len();
            if header_width >= 185.0 {
                ui.separator();
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
            if header_width >= 315.0 {
                ui.separator();
                if icon_button(ui, UiIcon::ZoomOut, "Zoom out").clicked() {
                    self.requested_zoom =
                        Some((self.zoom / 1.15).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM));
                    self.fit_width = false;
                }
                if icon_button(ui, UiIcon::ZoomIn, "Zoom in").clicked() {
                    self.requested_zoom =
                        Some((self.zoom * 1.15).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM));
                    self.fit_width = false;
                }
                if header_width >= 400.0 {
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
            .map(|page| page_stack_geometry([page.size], 1.0)[0].size.x)
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
                for (page, geometry) in self.pages.iter().zip(&geometries) {
                    let left = ((content_width - geometry.size.x) * 0.5).max(PAGE_MARGIN);
                    let rect = Rect::from_min_size(
                        Pos2::new(
                            ui.min_rect().left() + left,
                            ui.min_rect().top() + geometry.top,
                        ),
                        geometry.size,
                    );
                    let shadow_rect = rect.translate(Vec2::new(0.0, 3.0)).expand(2.0);
                    ui.painter().rect_filled(
                        shadow_rect,
                        3.0,
                        Color32::from_black_alpha(if self.preview_dark { 120 } else { 80 }),
                    );
                    let page_fill = if self.preview_dark {
                        Color32::from_rgb(20, 22, 28)
                    } else {
                        Color32::WHITE
                    };
                    ui.painter().rect_filled(rect, 1.0, page_fill);
                    ui.painter().rect_stroke(
                        rect,
                        1.0,
                        Stroke::new(
                            1.0,
                            Color32::from_gray(if self.preview_dark { 74 } else { 150 }),
                        ),
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
                        .expand(2.0)
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

    fn show_problems(&self, ui: &mut egui::Ui) {
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
        egui::ScrollArea::vertical().show(ui, |ui| {
            if self.diagnostics.is_empty() && self.tinymist_diagnostics.is_empty() {
                ui.label(RichText::new("No compiler diagnostics").weak());
            }
            for diagnostic in self.diagnostics.iter().chain(&self.tinymist_diagnostics) {
                let color = diagnostic_color(diagnostic.severity, ui.visuals().dark_mode);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(diagnostic.severity.label())
                            .strong()
                            .color(color),
                    );
                    if let Some(location) = diagnostic.location {
                        ui.label(
                            RichText::new(format!("{}:{}", location.line, location.column))
                                .monospace()
                                .color(ui.visuals().weak_text_color()),
                        );
                    }
                    ui.label(&diagnostic.message);
                });
                for detail in &diagnostic.details {
                    ui.label(RichText::new(detail).small().monospace().weak());
                }
            }
            if !self.raw_diagnostics.is_empty() {
                ui.collapsing("Raw Typst output", |ui| {
                    ui.label(RichText::new(&self.raw_diagnostics).monospace());
                });
            }
        });
    }

    fn show_status_bar(&self, ui: &mut egui::Ui) {
        let mut fallbacks = self.non_preview_fallback_details(ui.ctx().system_theme());
        if let Some(reason) = self.preview_fallback_reason() {
            fallbacks.insert(0, format!("Preview: {reason}"));
        }
        ui.horizontal(|ui| {
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
            native_hover_text(static_icon(ui, icon, color), self.status_detail());
            if let Some(timing) = timing {
                native_hover_text(
                    ui.label(RichText::new(timing).small().strong().color(color)),
                    self.status_detail(),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.document_kind.is_editable() {
                    // This is intentionally the first right-to-left item: its
                    // position is pinned to the window edge regardless of
                    // status or notice length.
                    ui.label(
                        RichText::new(format!(
                            "{} lines · {} chars",
                            logical_line_count(&self.source),
                            self.source.chars().count()
                        ))
                        .small(),
                    );
                }
                if !fallbacks.is_empty() {
                    ui.separator();
                    let count = fallbacks.len();
                    let color = warning_color(ui.visuals().dark_mode);
                    native_hover_text(
                        ui.label(
                            RichText::new(count.to_string())
                                .small()
                                .strong()
                                .color(color),
                        ),
                        fallbacks.join("\n"),
                    );
                    native_hover_text(
                        static_icon(ui, UiIcon::Warning, color),
                        fallbacks.join("\n"),
                    );
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
            let Some(window) = frame.winit_window() else {
                self.webview_state = ServiceState::Degraded(
                    "The native window handle is temporarily unavailable".to_owned(),
                );
                return false;
            };
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
            #[cfg(target_os = "windows")]
            let builder = builder.with_hotkeys_zoom(true);
            match builder.build_as_child(window.as_ref()) {
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
}

impl eframe::App for EditorApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn raw_input_hook(&mut self, context: &egui::Context, _raw_input: &mut egui::RawInput) {
        let Some(settings) = self.pending_settings.take() else {
            return;
        };
        let interface_theme_changed = settings.interface_theme != self.settings.interface_theme;
        let autosave_changed = settings.auto_save != self.settings.auto_save
            || settings.auto_save_delay_ms != self.settings.auto_save_delay_ms;
        self.settings = settings;
        if interface_theme_changed {
            context.set_theme(self.settings.interface_theme.egui_preference());
        }
        if autosave_changed {
            self.autosave_deadline =
                (self.settings.auto_save && self.path.is_some() && self.is_dirty()).then(|| {
                    Instant::now()
                        + Duration::from_millis(self.settings.auto_save_delay_ms.max(100))
                });
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.pending_settings
            .as_ref()
            .unwrap_or(&self.settings)
            .save(storage);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        // The GL surface is alpha-capable for child popup viewports. Keep the
        // main window itself fully opaque by painting its complete root first.
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        self.diagnostic_tooltip = None;
        context.data_mut(|data| data.remove::<HoverTooltipOverlay>(native_hover_tooltip_id()));
        self.execute_pending_app_popup_action(&context);
        self.execute_pending_document_action(&context);
        self.sync_runtime_settings(&context);
        self.receive_compile_results(&context);
        self.receive_asset_results(&context);
        self.poll_export_dialog(&context);
        self.poll_tool_picker(&context);
        self.poll_document_dialog(&context);
        self.receive_tinymist_events(&context);
        self.receive_web_links();
        self.handle_shortcuts(&context);
        self.handle_dropped_file(&context);
        self.handle_close_request(&context);
        self.update_title(&context);
        self.tick_workspace(&context);

        egui::Panel::top("toolbar")
            .exact_size(TOOLBAR_HEIGHT)
            .show(ui, |ui| self.show_toolbar(ui, frame));
        egui::Panel::bottom("status-bar")
            .exact_size(STATUS_HEIGHT)
            .show(ui, |ui| self.show_status_bar(ui));
        if self.problems_visible {
            egui::Panel::bottom("problems")
                .resizable(true)
                .default_size(140.0)
                .min_size(70.0)
                .max_size(320.0)
                .show(ui, |ui| self.show_problems(ui));
        }
        if self.filesystem_visible {
            egui::Panel::left("filesystem")
                .frame(compact_content_panel_frame(ui.style()))
                .resizable(true)
                .default_size(230.0)
                .min_size(0.0)
                .max_size(420.0)
                .show(ui, |ui| self.show_workspace(ui));
        }
        if self.document_kind.preview_only() {
            // This presentation override intentionally does not mutate
            // `view_mode`: returning to a source file restores the user's Code,
            // Split, or Preview preference.
            egui::CentralPanel::default()
                .frame(compact_content_panel_frame(ui.style()))
                .show(ui, |ui| self.show_preview(ui, frame));
        } else if self.document_kind == DocumentKind::Text {
            self.hide_webview();
            egui::CentralPanel::default()
                .frame(compact_content_panel_frame(ui.style()))
                .show(ui, |ui| self.show_editor(ui));
        } else {
            match self.view_mode {
                ViewMode::Code => {
                    self.hide_webview();
                    egui::CentralPanel::default()
                        .frame(compact_content_panel_frame(ui.style()))
                        .show(ui, |ui| self.show_editor(ui));
                }
                ViewMode::Split => {
                    let available = ui.available_width();
                    let preview_reserve = 96.0_f32.min((available * 0.48).max(40.0));
                    let max_editor = (available - preview_reserve).max(40.0);
                    let min_editor = 100.0_f32.min(max_editor);
                    let editor_width = (available * 0.52).clamp(min_editor, max_editor);
                    egui::Panel::left("editor")
                        .frame(compact_content_panel_frame(ui.style()))
                        .resizable(true)
                        .default_size(editor_width)
                        .min_size(min_editor)
                        .max_size(max_editor)
                        .show(ui, |ui| self.show_editor(ui));
                    egui::CentralPanel::default()
                        .frame(compact_content_panel_frame(ui.style()))
                        .show(ui, |ui| self.show_preview(ui, frame));
                }
                ViewMode::Preview => {
                    egui::CentralPanel::default()
                        .frame(compact_content_panel_frame(ui.style()))
                        .show(ui, |ui| self.show_preview(ui, frame));
                }
            }
        }
        self.show_app_popup_window(&context);
        self.show_rename_dialog(&context);
        self.show_app_modal_window(&context);
        self.show_diagnostic_tooltip_window(&context);
        self.show_settings_window(&context);
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

fn take_matching_export(pending: &mut Option<PendingExport>, revision: u64) -> Option<PathBuf> {
    pending
        .take()
        .filter(|export| export.revision == revision)
        .map(|export| export.path)
}

fn paint_editor_line_backgrounds(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    source: &str,
    current_char: Option<usize>,
    attention: Option<(usize, f32)>,
    slot: egui::layers::ShapeIdx,
) {
    let line_rows = logical_line_row_ranges(&output.galley.rows);
    let mut shapes = Vec::new();

    if let Some(char_index) = current_char {
        let line = line_index_at_char(source, char_index);
        let fill = if ui.visuals().dark_mode {
            Color32::from_rgba_unmultiplied(91, 143, 190, 25)
        } else {
            Color32::from_rgba_unmultiplied(55, 122, 181, 18)
        };
        push_editor_line_fill(&mut shapes, output, &line_rows, line, fill, None);
    }

    if let Some((char_index, progress)) = attention {
        let cursor = output
            .galley
            .pos_from_cursor(CCursor::new(char_index.min(source.chars().count())))
            .translate(output.galley_pos.to_vec2());
        let center = Pos2::new(cursor.left(), cursor.center().y);
        let fade = (1.0 - progress).powi(2);
        let radius = 18.0 + 46.0 * progress;
        let center_color = if ui.visuals().dark_mode {
            Color32::from_rgba_unmultiplied(74, 196, 235, (105.0 * fade) as u8)
        } else {
            Color32::from_rgba_unmultiplied(18, 132, 193, (82.0 * fade) as u8)
        };
        let mut mesh = egui::epaint::Mesh::default();
        mesh.colored_vertex(center, center_color);
        const SEGMENTS: u32 = 28;
        for index in 0..SEGMENTS {
            let angle = index as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            mesh.colored_vertex(center + Vec2::angled(angle) * radius, Color32::TRANSPARENT);
        }
        for index in 0..SEGMENTS {
            mesh.add_triangle(0, index + 1, ((index + 1) % SEGMENTS) + 1);
        }
        shapes.push(egui::Shape::mesh(mesh));
        let ring_color = center_color.gamma_multiply(0.72);
        shapes.push(egui::Shape::circle_stroke(
            center,
            7.0 + 34.0 * progress,
            Stroke::new(1.2, ring_color),
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
            Pos2::new(combined.left() + 3.0, combined.bottom()),
        );
        shapes.push(egui::Shape::rect_filled(marker_rect, 1.5, marker));
    }
}

fn line_index_at_char(source: &str, char_index: usize) -> usize {
    source
        .chars()
        .take(char_index)
        .filter(|character| *character == '\n')
        .count()
}

fn editor_attention_progress(elapsed: Duration) -> f32 {
    (elapsed.as_secs_f32() / EDITOR_ATTENTION_DURATION.as_secs_f32()).clamp(0.0, 1.0)
}

fn paint_line_diagnostics(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    diagnostics: &[LineDiagnostic],
    slots: Vec<egui::layers::ShapeIdx>,
) -> Option<DiagnosticTooltipOverlay> {
    let painter = ui.painter();
    let line_rows = logical_line_row_ranges(&output.galley.rows);
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
                color.gamma_multiply(0.15),
            ));
        }
        painter.set(slot, egui::Shape::Vec(background));

        let Some(row) = output.galley.rows.get(rows.end.saturating_sub(1)) else {
            continue;
        };
        let row_rect = row.rect().translate(output.galley_pos.to_vec2());
        let text = format!("  {}", diagnostic.summary);
        let galley = painter.layout_no_wrap(text, FontId::monospace(12.5), color);
        let annotation_pos = Pos2::new(
            output.galley_pos.x + row.rect().right() + 8.0,
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
        if response.hovered() && hovered_diagnostic.is_none() {
            hovered_diagnostic = Some(DiagnosticTooltipOverlay {
                anchor: Pos2::new(output.response.rect.right() + 6.0, hover_rect.top()),
                severity: diagnostic.severity,
                detail: diagnostic.detail.clone(),
            });
        }
    }
    hovered_diagnostic
}

fn paint_line_numbers(ui: &mut egui::Ui, output: &egui::text_edit::TextEditOutput) {
    let painter = ui.painter();
    let right = output.galley_pos.x - 9.0;
    let separator_x = output.galley_pos.x - 5.0;
    let separator = Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
    painter.line_segment(
        [
            Pos2::new(separator_x, output.response.rect.top()),
            Pos2::new(separator_x, output.response.rect.bottom()),
        ],
        separator,
    );
    let color = ui.visuals().weak_text_color();
    for (line, rows) in logical_line_row_ranges(&output.galley.rows)
        .into_iter()
        .enumerate()
    {
        let Some(row) = output.galley.rows.get(rows.start) else {
            continue;
        };
        let row_rect = row.rect().translate(output.galley_pos.to_vec2());
        painter.text(
            Pos2::new(right, row_rect.top()),
            egui::Align2::RIGHT_TOP,
            (line + 1).to_string(),
            FontId::monospace(12.5),
            color,
        );
    }
}

fn logical_line_count(source: &str) -> usize {
    source.bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn line_number_gutter_width(line_count: usize, enabled: bool) -> i8 {
    if !enabled {
        return 4;
    }
    let digits = line_count.max(1).ilog10() + 1;
    ((digits * 9 + 18).min(120)) as i8
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
) {
    for node in nodes {
        let is_active = active.is_some_and(|path| same_path(path, &node.path));
        let label = node.display_name().into_owned();
        if node.is_directory() {
            let open = builder.node(
                NodeBuilder::dir(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, true))
                    .label_ui(move |ui| {
                        let text = RichText::new(&label);
                        ui.label(if is_active { text.strong() } else { text });
                    }),
            );
            if open {
                add_workspace_nodes(builder, &node.children, active);
            }
            builder.close_dir();
        } else if node.is_file() {
            builder.node(
                NodeBuilder::leaf(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, false))
                    .label_ui(move |ui| {
                        let text = RichText::new(&label);
                        ui.label(if is_active { text.strong() } else { text });
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

fn paint_tree_icon(ui: &mut egui::Ui, folder: bool) {
    let rect = ui.available_rect_before_wrap().shrink(2.0);
    let size = Vec2::new(14.0, 12.0).min(rect.size());
    let rect = Rect::from_center_size(rect.center(), size);
    let color = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let stroke = Stroke::new(1.2, color);
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
            egui::Button::new("").min_size(Vec2::new(22.0, 20.0)),
        ),
        tooltip,
    );
    let color = ui.style().interact(&response).fg_stroke.color;
    paint_ui_icon(ui.painter(), response.rect.shrink(4.0), icon, color);
    response
}

fn static_icon(ui: &mut egui::Ui, icon: UiIcon, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
    paint_ui_icon(ui.painter(), rect.shrink(1.0), icon, color);
    response
}

fn paint_ui_icon(painter: &egui::Painter, rect: Rect, icon: UiIcon, color: Color32) {
    let center = rect.center();
    let stroke = Stroke::new(1.5, color);
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
        UiIcon::Refresh | UiIcon::Waiting => {
            painter.circle_stroke(center, rect.width().min(rect.height()) * 0.36, stroke);
            if icon == UiIcon::Refresh {
                painter.line_segment(
                    [
                        Pos2::new(rect.right() - 4.0, rect.top() + 1.0),
                        Pos2::new(rect.right() - 1.0, rect.top() + 5.0),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        Pos2::new(rect.right() - 1.0, rect.top() + 5.0),
                        Pos2::new(rect.right() - 6.0, rect.top() + 5.0),
                    ],
                    stroke,
                );
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

fn configure_theme_styles(context: &egui::Context) {
    context.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(7.0, 3.0);
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            FontId::new(15.0, FontFamily::Monospace),
        );
    });
}

fn compact_content_panel_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(egui::Margin::symmetric(8, 0))
}

fn popup_card_frame(style: &egui::Style) -> egui::Frame {
    // Native overlay viewports already provide alpha outside the card. The
    // stock popup shadow is a dark rectangular raster at those rounded edges,
    // so leave the shadow to the card's border and keep every corner clear.
    egui::Frame::popup(style).shadow(egui::epaint::Shadow::NONE)
}

fn show_panel_header(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> Rect {
    // `allocate_ui_with_layout` sizes itself from its children, which made
    // labels, small buttons, and selectable labels produce three different
    // header heights. A nested exact-size panel owns its geometry, clips
    // overflowing children, and advances the body directly to its separator.
    let fill = ui.visuals().panel_fill;
    let available = ui.available_rect_before_wrap();
    let response = egui::Panel::top(ui.id().with(id_salt))
        .exact_size(PANEL_HEADER_HEIGHT)
        .frame(egui::Frame::NONE.fill(fill))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
            ui.with_layout(Layout::left_to_right(Align::Center), add_contents);
        });
    response.response.rect.intersect(available)
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
    if dark_mode {
        Color32::from_rgb(237, 135, 150)
    } else {
        Color32::from_rgb(176, 36, 55)
    }
}

fn warning_color(dark_mode: bool) -> Color32 {
    if dark_mode {
        Color32::from_rgb(238, 212, 159)
    } else {
        Color32::from_rgb(145, 91, 10)
    }
}

fn info_color(dark_mode: bool) -> Color32 {
    if dark_mode {
        Color32::from_rgb(125, 196, 228)
    } else {
        Color32::from_rgb(0, 102, 148)
    }
}

fn success_color(dark_mode: bool) -> Color32 {
    if dark_mode {
        Color32::from_rgb(166, 218, 149)
    } else {
        Color32::from_rgb(43, 120, 48)
    }
}

fn neutral_color(dark_mode: bool) -> Color32 {
    if dark_mode {
        Color32::from_rgb(166, 173, 186)
    } else {
        Color32::from_rgb(82, 88, 99)
    }
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

fn settings_value_row(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        settings_inline_value(ui, name, value);
    });
}

fn settings_inline_value(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.label(RichText::new(format!("{name}:")).weak());
    ui.label(value);
}

fn tool_preference_editor(ui: &mut egui::Ui, label: &str, preference: &mut ToolPreference) -> bool {
    let mut browse = false;
    ui.push_id(label, |ui| {
        ui.label(RichText::new(label).strong());
        ui.horizontal_wrapped(|ui| {
            for mode in ToolMode::ALL {
                ui.selectable_value(&mut preference.mode, mode, mode.label());
            }
        });
        if preference.mode == ToolMode::Custom {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut preference.custom_path)
                        .hint_text("/absolute/path/to/executable")
                        .desired_width(270.0),
                );
                browse |= ui.button("Browse…").clicked();
            });
        }
    });
    browse
}

fn show_tool_resolution(ui: &mut egui::Ui, resolution: &ToolResolution) {
    let color = match resolution.origin {
        ToolOrigin::Bundled | ToolOrigin::Custom => success_color(ui.visuals().dark_mode),
        ToolOrigin::Environment | ToolOrigin::Path => warning_color(ui.visuals().dark_mode),
        ToolOrigin::Missing => error_color(ui.visuals().dark_mode),
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Effective:").weak());
        ui.label(
            RichText::new(resolution.origin.label())
                .strong()
                .color(color),
        );
        ui.label(
            RichText::new(resolution.program.display().to_string())
                .small()
                .monospace(),
        );
    });
    if let Some(reason) = &resolution.fallback_reason {
        fallback_notice(ui, "BINARY FALLBACK ACTIVE", reason);
    }
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
    let response = egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(name).strong());
                ui.label(RichText::new(status).small().strong().color(color));
            });
        })
        .response;
    response.on_hover_text(detail);
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
        "Tinymist SVG"
    } else if preference == PreviewPreference::Interactive {
        "Native PDF · fallback"
    } else {
        "Native PDF"
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
    if let Some(provider) = diagnostic.source {
        details.push(format!("source: {provider}"));
    }
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

fn lsp_range_to_char_range(source: &str, range: &LspRange) -> Range<usize> {
    let start = lsp_position_to_char(source, &range.start);
    let end = lsp_position_to_char(source, &range.end).max(start);
    start..end
}

fn lsp_position_to_char(source: &str, position: &LspPosition) -> usize {
    let target_line = position.line as usize;
    let target_utf16 = position.character as usize;
    let mut absolute_chars = 0;
    for (line_index, segment) in source.split_inclusive('\n').enumerate() {
        if line_index == target_line {
            let line = segment.strip_suffix('\n').unwrap_or(segment);
            let mut utf16 = 0;
            for character in line.chars() {
                let next = utf16 + character.len_utf16();
                if next > target_utf16 {
                    break;
                }
                utf16 = next;
                absolute_chars += 1;
            }
            return absolute_chars;
        }
        absolute_chars += segment.chars().count();
    }
    source.chars().count()
}

#[derive(Debug, Clone, Copy)]
struct ResolvedLspTextEdit<'a> {
    start_byte: usize,
    end_byte: usize,
    start_char: usize,
    end_char: usize,
    replacement: &'a str,
}

fn resolve_lsp_text_edits<'a>(
    source: &str,
    edits: &'a [LspTextEdit],
) -> Result<Vec<ResolvedLspTextEdit<'a>>, String> {
    let mut resolved = Vec::with_capacity(edits.len());
    for edit in edits {
        let start_byte = strict_lsp_position_to_byte(source, &edit.range.start)?;
        let end_byte = strict_lsp_position_to_byte(source, &edit.range.end)?;
        if start_byte > end_byte {
            return Err("an edit range ends before it starts".to_owned());
        }
        resolved.push(ResolvedLspTextEdit {
            start_byte,
            end_byte,
            start_char: source[..start_byte].chars().count(),
            end_char: source[..end_byte].chars().count(),
            replacement: &edit.new_text,
        });
    }
    resolved.sort_by_key(|edit| (edit.start_byte, edit.end_byte));
    for pair in resolved.windows(2) {
        if pair[1].start_byte < pair[0].end_byte {
            return Err("formatting edits overlap".to_owned());
        }
    }
    Ok(resolved)
}

fn apply_resolved_lsp_text_edits(source: &str, resolved: &[ResolvedLspTextEdit<'_>]) -> String {
    let mut formatted = source.to_owned();
    for edit in resolved.iter().rev() {
        formatted.replace_range(edit.start_byte..edit.end_byte, edit.replacement);
    }
    formatted
}

fn map_char_index_through_lsp_edits(
    source_len: usize,
    index: usize,
    resolved: &[ResolvedLspTextEdit<'_>],
) -> usize {
    let index = index.min(source_len);
    let mut delta = 0_isize;
    for edit in resolved {
        if index < edit.start_char {
            break;
        }
        let replacement_len = edit.replacement.chars().count();
        if edit.start_char == edit.end_char {
            delta += replacement_len as isize;
            continue;
        }
        let shifted_start = (edit.start_char as isize + delta).max(0) as usize;
        if index == edit.start_char {
            return shifted_start;
        }
        if index < edit.end_char {
            return shifted_start + replacement_len;
        }
        delta += replacement_len as isize - (edit.end_char - edit.start_char) as isize;
    }
    (index as isize + delta).max(0) as usize
}

fn apply_lsp_text_edits_and_map_cursor(
    source: &str,
    edits: &[LspTextEdit],
    cursor: CCursorRange,
) -> Result<(String, CCursorRange), String> {
    let resolved = resolve_lsp_text_edits(source, edits)?;
    let source_len = source.chars().count();
    let mapped = CCursorRange {
        primary: CCursor::new(map_char_index_through_lsp_edits(
            source_len,
            cursor.primary.index.0,
            &resolved,
        )),
        secondary: CCursor::new(map_char_index_through_lsp_edits(
            source_len,
            cursor.secondary.index.0,
            &resolved,
        )),
        h_pos: cursor.h_pos,
    };
    Ok((apply_resolved_lsp_text_edits(source, &resolved), mapped))
}

#[cfg(test)]
fn apply_lsp_text_edits(source: &str, edits: &[LspTextEdit]) -> Result<String, String> {
    let resolved = resolve_lsp_text_edits(source, edits)?;
    Ok(apply_resolved_lsp_text_edits(source, &resolved))
}

fn strict_lsp_position_to_byte(source: &str, position: &LspPosition) -> Result<usize, String> {
    let mut line_start = 0;
    for _ in 0..position.line {
        let Some(relative_newline) = source[line_start..].find('\n') else {
            return Err(format!("line {} is outside the document", position.line));
        };
        line_start += relative_newline + 1;
    }
    let rest = &source[line_start..];
    let mut line = rest.split_once('\n').map_or(rest, |(line, _)| line);
    if let Some(stripped) = line.strip_suffix('\r') {
        line = stripped;
    }
    let target = position.character as usize;
    let mut utf16 = 0;
    for (byte, character) in line.char_indices() {
        if utf16 == target {
            return Ok(line_start + byte);
        }
        let next = utf16 + character.len_utf16();
        if target < next {
            return Err(format!(
                "UTF-16 column {} splits a surrogate pair on line {}",
                position.character, position.line
            ));
        }
        utf16 = next;
    }
    if utf16 == target {
        Ok(line_start + line.len())
    } else {
        Err(format!(
            "UTF-16 column {} is outside line {}",
            position.character, position.line
        ))
    }
}

fn source_position_at_char(source: &str, char_index: usize) -> (u32, u32) {
    let mut line = 0_u32;
    let mut character = 0_u32;
    for value in source.chars().take(char_index) {
        if value == '\n' {
            line = line.saturating_add(1);
            character = 0;
        } else {
            character = character.saturating_add(1);
        }
    }
    (line, character)
}

fn approximate_char_capacity(width: f32, font_size: f32) -> usize {
    (width.max(0.0) / (font_size * 0.56).max(1.0)).floor() as usize
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

fn source_editor_id() -> egui::Id {
    egui::Id::new("tiptoptyp-source-editor")
}

fn native_hover_tooltip_id() -> egui::Id {
    egui::Id::new("tiptoptyp-native-hover-tooltip")
}

fn native_hover_text(response: egui::Response, detail: impl Into<String>) -> egui::Response {
    if response.hovered() {
        let tooltip = HoverTooltipOverlay {
            anchor: response.rect.left_bottom() + egui::vec2(0.0, 4.0),
            detail: detail.into(),
        };
        response
            .ctx
            .data_mut(|data| data.insert_temp(native_hover_tooltip_id(), tooltip));
    }
    response
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
        Some(shortcut) => {
            egui::Button::new(label).shortcut_text(ui.ctx().format_shortcut(&shortcut))
        }
        None => egui::Button::new(label),
    }
}

fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: Option<KeyboardShortcut>) -> egui::Response {
    ui.add(menu_button(ui, label, shortcut))
}

fn show_file_popup_ui(ui: &mut egui::Ui, action: &mut Option<AppPopupAction>) {
    let entries = [
        (
            "New",
            KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::N),
            FileMenuAction::New,
        ),
        (
            "Open…",
            KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::O),
            FileMenuAction::Open,
        ),
        (
            "Open Folder…",
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::O),
            FileMenuAction::OpenFolder,
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
    if ui
        .add_enabled(
            can_undo,
            menu_button(
                ui,
                "Undo",
                Some(KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Z)),
            ),
        )
        .clicked()
    {
        *action = Some(AppPopupAction::Editor(EditorMenuAction::Undo));
    }
    if ui
        .add_enabled(
            can_redo,
            menu_button(
                ui,
                "Redo",
                Some(KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::SHIFT,
                    egui::Key::Z,
                )),
            ),
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
    if ui
        .add_enabled(
            can_format,
            menu_button(ui, "Format Document", Some(format_shortcut())),
        )
        .clicked()
    {
        *action = Some(AppPopupAction::Editor(EditorMenuAction::Format));
    }
}

fn show_workspace_popup_ui(
    ui: &mut egui::Ui,
    path: &Path,
    is_file: bool,
    action: &mut Option<AppPopupAction>,
) {
    if ui.add_enabled(is_file, egui::Button::new("Open")).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Open(
            path.to_path_buf(),
        )));
    }
    if ui
        .add_enabled(is_file, egui::Button::new("Rename…"))
        .clicked()
    {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Rename(
            path.to_path_buf(),
        )));
    }
    ui.separator();
    if ui.button("Copy Path").clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::CopyPath(
            path.to_path_buf(),
        )));
    }
    if ui.button(reveal_label()).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Reveal(
            path.to_path_buf(),
        )));
    }
}

fn clamp_popup_anchor(anchor: Pos2, popup_size: Vec2, viewport_size: Vec2) -> Pos2 {
    let margin = 4.0;
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
    if ui
        .add_enabled(can_undo, egui::Button::new("Undo"))
        .clicked()
    {
        *action = Some(EditorMenuAction::Undo);
        ui.close();
    }
    if ui
        .add_enabled(can_redo, egui::Button::new("Redo"))
        .clicked()
    {
        *action = Some(EditorMenuAction::Redo);
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(has_selection, egui::Button::new("Cut"))
        .clicked()
    {
        *action = Some(EditorMenuAction::Cut);
        ui.close();
    }
    if ui
        .add_enabled(has_selection, egui::Button::new("Copy"))
        .clicked()
    {
        *action = Some(EditorMenuAction::Copy);
        ui.close();
    }
    if ui.button("Paste").clicked() {
        *action = Some(EditorMenuAction::Paste);
        ui.close();
    }
    if ui.button("Select All").clicked() {
        *action = Some(EditorMenuAction::SelectAll);
        ui.close();
    }
    if can_format {
        ui.separator();
        if ui.button("Format Document").clicked() {
            *action = Some(EditorMenuAction::Format);
            ui.close();
        }
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
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let existing_permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        format!(
            "Could not create a temporary file beside {}: {error}",
            path.display()
        )
    })?;
    temporary
        .write_all(contents)
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("Could not flush {}: {error}", path.display()))?;
    if let Some(permissions) = existing_permissions {
        temporary
            .as_file()
            .set_permissions(permissions)
            .map_err(|error| {
                format!(
                    "Could not preserve permissions for {}: {error}",
                    path.display()
                )
            })?;
    }
    temporary
        .persist(path)
        .map_err(|error| format!("Could not replace {}: {}", path.display(), error.error))?;
    Ok(())
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
    fn queued_exports_never_cross_document_revisions() {
        let mut pending = Some(PendingExport {
            path: PathBuf::from("old-document.pdf"),
            revision: 7,
        });
        assert_eq!(take_matching_export(&mut pending, 8), None);
        assert!(pending.is_none());

        let mut pending = Some(PendingExport {
            path: PathBuf::from("current-document.pdf"),
            revision: 8,
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
    fn panel_headers_have_exact_and_gapless_geometry_for_varied_controls() {
        let context = egui::Context::default();
        configure_theme_styles(&context);
        let style = context.style_of(context.theme());
        let frame = compact_content_panel_frame(&style);
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
                        let header = show_panel_header(ui, id, |ui| {
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
                (header.height() - PANEL_HEADER_HEIGHT).abs() < 0.01,
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
                                    show_panel_header(ui, "narrow-explorer-header-test", |ui| {
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
                (header.height() - PANEL_HEADER_HEIGHT).abs() < 0.01
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
    fn view_modes_have_the_requested_panel_matrix() {
        assert!(ViewMode::Code.shows_code());
        assert!(!ViewMode::Code.shows_preview());
        assert!(ViewMode::Split.shows_code());
        assert!(ViewMode::Split.shows_preview());
        assert!(!ViewMode::Preview.shows_code());
        assert!(ViewMode::Preview.shows_preview());
    }

    #[test]
    fn lsp_utf16_positions_map_to_egui_scalar_offsets() {
        let source = "a🦀b\nsecond";
        assert_eq!(
            lsp_position_to_char(
                source,
                &LspPosition {
                    line: 0,
                    character: 3,
                }
            ),
            2
        );
        assert_eq!(
            lsp_position_to_char(
                source,
                &LspPosition {
                    line: 1,
                    character: 3,
                }
            ),
            7
        );
    }

    #[test]
    fn formatting_edits_are_utf16_strict_atomic_and_unicode_safe() {
        let source = "a🦀b\nsecond";
        let edits = vec![LspTextEdit {
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 1,
                },
                end: LspPosition {
                    line: 0,
                    character: 3,
                },
            },
            new_text: "crab".to_owned(),
        }];
        assert_eq!(
            apply_lsp_text_edits(source, &edits).unwrap(),
            "acrabb\nsecond"
        );

        let splits_surrogate = vec![LspTextEdit {
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 2,
                },
                end: LspPosition {
                    line: 0,
                    character: 3,
                },
            },
            new_text: String::new(),
        }];
        assert!(apply_lsp_text_edits(source, &splits_surrogate).is_err());
        assert_eq!(source, "a🦀b\nsecond");
    }

    #[test]
    fn formatting_edits_map_unicode_cursor_and_selection() {
        let source = "a🦀b";
        let edits = vec![
            LspTextEdit {
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        character: 1,
                    },
                    end: LspPosition {
                        line: 0,
                        character: 1,
                    },
                },
                new_text: " ".to_owned(),
            },
            LspTextEdit {
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        character: 3,
                    },
                    end: LspPosition {
                        line: 0,
                        character: 4,
                    },
                },
                new_text: "bee".to_owned(),
            },
        ];
        let cursor = CCursorRange::two(CCursor::new(1), CCursor::new(3));
        let (formatted, mapped) =
            apply_lsp_text_edits_and_map_cursor(source, &edits, cursor).unwrap();

        assert_eq!(formatted, "a 🦀bee");
        assert_eq!(mapped.primary.index.0, 6);
        assert_eq!(mapped.secondary.index.0, 2);
    }

    #[test]
    fn formatting_cursor_inside_replacement_moves_to_replacement_end() {
        let source = "abcd";
        let edits = vec![LspTextEdit {
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 0,
                },
                end: LspPosition {
                    line: 0,
                    character: 3,
                },
            },
            new_text: "xy".to_owned(),
        }];
        let cursor = CCursorRange::one(CCursor::new(2));
        let (formatted, mapped) =
            apply_lsp_text_edits_and_map_cursor(source, &edits, cursor).unwrap();

        assert_eq!(formatted, "xyd");
        assert_eq!(mapped.primary.index.0, 2);
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
    fn egui_scalar_offsets_map_to_tinymist_preview_source_positions() {
        let source = "a🦀b\nsecond";
        assert_eq!(source_position_at_char(source, 2), (0, 2));
        assert_eq!(source_position_at_char(source, 7), (1, 3));
        assert_eq!(source_position_at_char(source, usize::MAX), (1, 6));
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
        assert_eq!(editor_attention_progress(EDITOR_ATTENTION_DURATION), 1.0);
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
                                    FontId::monospace(15.0),
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
    fn system_theme_tracks_os_events_but_explicit_choices_do_not() {
        let context = egui::Context::default();
        context.set_theme(egui::ThemePreference::System);
        context
            .run_ui(
                egui::RawInput {
                    system_theme: Some(egui::Theme::Light),
                    ..Default::default()
                },
                |_| {},
            )
            .drop_without_applying_deltas();
        assert_eq!(context.theme(), egui::Theme::Light);

        context
            .run_ui(
                egui::RawInput {
                    system_theme: Some(egui::Theme::Dark),
                    ..Default::default()
                },
                |_| {},
            )
            .drop_without_applying_deltas();
        assert_eq!(context.theme(), egui::Theme::Dark);

        context.set_theme(egui::ThemePreference::Dark);
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
    fn light_and_dark_styles_have_identical_layout_geometry() {
        let context = egui::Context::default();
        configure_theme_styles(&context);
        let dark = context.style_of(egui::Theme::Dark);
        let light = context.style_of(egui::Theme::Light);

        assert_eq!(dark.spacing, light.spacing);
        assert_eq!(dark.text_styles, light.text_styles);
        assert_eq!(dark.spacing.item_spacing, egui::vec2(6.0, 4.0));
        assert_eq!(dark.spacing.button_padding, egui::vec2(7.0, 3.0));
        assert_eq!(
            dark.text_styles.get(&egui::TextStyle::Monospace),
            Some(&FontId::new(15.0, FontFamily::Monospace))
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
            "Native PDF · fallback"
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
            "Native PDF"
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
        assert_eq!(
            converted.full_message(),
            "old syntax\nsource: tinymist\ncode: \"deprecated\""
        );
    }
}
