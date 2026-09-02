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
    text::{CCursor, CCursorRange, LayoutJob, TextFormat},
};
use egui_ltreeview::{Action as TreeAction, TreeView, TreeViewBuilder};
use rfd::{
    AsyncFileDialog, FileDialog, FileHandle, MessageButtons, MessageDialog, MessageDialogResult,
    MessageLevel,
};

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
        LspRange, TextDocument, TinymistConfig, TinymistDiagnostic, TinymistEvent, TinymistSidecar,
    },
    toolchain::{ToolKind, ToolOrigin, ToolResolution, resolve_tool},
    workspace::{WorkspaceNode, WorkspaceTree},
};

const COMPILE_DEBOUNCE: Duration = Duration::from_millis(60);
const WORKSPACE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const TOOLBAR_HEIGHT: f32 = 30.0;
const STATUS_HEIGHT: f32 = 24.0;
const PANEL_HEADER_HEIGHT: f32 = 28.0;
const SETTINGS_PANEL_WIDTH: f32 = 390.0;
const MIN_PREVIEW_ZOOM: f32 = 0.2;
const MAX_PREVIEW_ZOOM: f32 = 6.0;
const EDITOR_ATTENTION_DURATION: Duration = Duration::from_millis(900);

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
enum NoticeKind {
    Info,
    Success,
    Error,
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

struct PendingExport {
    path: PathBuf,
    revision: u64,
}

pub struct EditorApp {
    source: String,
    path: Option<PathBuf>,
    revision: u64,
    saved_revision: u64,
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

    pending_export: Option<PendingExport>,
    pending_export_dialog: Option<Pin<Box<dyn Future<Output = Option<FileHandle>>>>>,
    notice: Option<Notice>,
    last_title: String,
    allow_close: bool,

    tinymist: TinymistSidecar,
    tinymist_generation: Option<Generation>,
    tinymist_uri: Option<String>,
    tinymist_url: Option<String>,
    tinymist_state: ServiceState,
    webview_state: ServiceState,
    fallback_history: Vec<String>,

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
            path: None,
            revision: 0,
            saved_revision: 0,
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
            pending_export: None,
            pending_export_dialog: None,
            notice: None,
            last_title: String::new(),
            allow_close: false,
            tinymist: TinymistSidecar::new(context.egui_ctx.clone()),
            tinymist_generation: None,
            tinymist_uri: None,
            tinymist_url: None,
            tinymist_state: ServiceState::Starting("Launching Tinymist LSP".to_owned()),
            webview_state: ServiceState::Starting(
                "Waiting for Tinymist's preview server".to_owned(),
            ),
            fallback_history: Vec::new(),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview_url: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            web_link_sender,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            web_link_receiver,
        };

        if let Some(path) = initial_path {
            app.load_path(path);
        } else {
            app.reset_document_services();
        }
        app
    }

    fn is_dirty(&self) -> bool {
        self.document_kind.is_editable() && self.revision != self.saved_revision
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
            .or_else(|| std::env::current_dir().ok())
    }

    fn project_root(&self) -> PathBuf {
        self.current_directory()
            .map(|directory| discover_project_root(&directory))
            .unwrap_or_else(|| PathBuf::from("."))
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
            project_root: discover_project_root(&source_dir),
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
        let new = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::N);
        let refresh = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::R);
        let find = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::F);
        let settings = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Comma);
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
        if context.input_mut(|input| input.consume_shortcut(&new)) {
            self.new_document();
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
        if let Some(path) = path
            && self.confirm_before_replacing("opening another document")
        {
            self.load_path(path);
        }
    }

    fn handle_close_request(&mut self, context: &egui::Context) {
        let close_requested = context.input(|input| input.viewport().close_requested());
        if close_requested && self.is_dirty() && !self.allow_close {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.confirm_before_replacing("closing tiptoptyp") {
                self.allow_close = true;
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn new_document(&mut self) {
        if !self.confirm_before_replacing("creating a new document") {
            return;
        }
        self.source = DEFAULT_SOURCE.to_owned();
        self.path = None;
        self.disk_fingerprint = None;
        self.document_kind = DocumentKind::Typst;
        self.revision = self.revision.wrapping_add(1);
        self.saved_revision = self.revision;
        self.clear_preview_for_document();
        self.search.clear();
        self.reset_document_services();
        self.schedule_compile_now();
    }

    fn open_dialog(&mut self) {
        if !self.confirm_before_replacing("opening another document") {
            return;
        }
        let mut dialog = FileDialog::new()
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
        if let Some(path) = dialog.pick_file() {
            self.load_path(path);
        }
    }

    fn load_path(&mut self, path: PathBuf) {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.show_file_error(format!("Could not open {}: {error}", path.display()));
                return;
            }
        };
        let kind = match DocumentKind::detect(&path, &bytes) {
            Ok(kind) => kind,
            Err(error) => {
                self.show_file_error(error);
                return;
            }
        };
        let source = if kind.is_editable() {
            // DocumentKind::detect already validated UTF-8.
            String::from_utf8(bytes.clone()).expect("validated UTF-8 document")
        } else {
            String::new()
        };
        let path = path.canonicalize().unwrap_or(path);

        self.document_kind = kind;
        self.disk_fingerprint = kind.is_editable().then(|| fingerprint(&bytes));
        self.source = source;
        self.path = Some(path.clone());
        self.revision = self.revision.wrapping_add(1);
        self.saved_revision = self.revision;
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
        let mut dialog = FileDialog::new().set_file_name(self.document_name());
        dialog = if self.document_kind.is_typst() {
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
        let Some(mut path) = dialog.save_file() else {
            return false;
        };
        if self.document_kind.is_typst() && path.extension().is_none() {
            path.set_extension("typ");
        }
        self.save_to(path)
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        let path_changed = self.path.as_ref() != Some(&path);
        if !path_changed && !self.confirm_disk_unchanged(&path) {
            return false;
        }
        match atomic_write(&path, self.source.as_bytes()) {
            Ok(()) => {
                self.path = Some(path.canonicalize().unwrap_or(path));
                self.disk_fingerprint = Some(fingerprint(self.source.as_bytes()));
                if path_changed {
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
                self.saved_revision = self.revision;
                true
            }
            Err(error) => {
                self.show_file_error(error);
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

        MessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title("File changed on disk")
            .set_description(description)
            .set_buttons(MessageButtons::YesNo)
            .show()
            == MessageDialogResult::Yes
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

    fn confirm_before_replacing(&mut self, action: &str) -> bool {
        if !self.is_dirty() {
            return true;
        }
        let result = MessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title("Unsaved changes")
            .set_description(format!(
                "Save changes to {} before {action}?",
                self.document_name()
            ))
            .set_buttons(MessageButtons::YesNoCancel)
            .show();
        match result {
            MessageDialogResult::Yes => self.save_document(),
            MessageDialogResult::No => true,
            MessageDialogResult::Cancel
            | MessageDialogResult::Ok
            | MessageDialogResult::Custom(_) => false,
        }
    }

    fn show_file_error(&mut self, message: String) {
        self.notice = Some(Notice {
            message: message.clone(),
            kind: NoticeKind::Error,
        });
        MessageDialog::new()
            .set_level(MessageLevel::Error)
            .set_title("tiptoptyp")
            .set_description(message)
            .set_buttons(MessageButtons::Ok)
            .show();
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
        if self.settings.preview_preference == PreviewPreference::Native {
            self.tinymist_state = ServiceState::Disabled("Native watched PDF selected".to_owned());
            self.webview_state =
                ServiceState::Disabled("Interactive preview is not requested".to_owned());
            return;
        }
        if !cfg!(any(target_os = "macos", target_os = "windows")) {
            let reason = "Embedded Tinymist preview is currently available on macOS and Windows";
            self.tinymist_state = ServiceState::Unsupported(reason.to_owned());
            self.webview_state = ServiceState::Unsupported(reason.to_owned());
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
        self.webview_state =
            ServiceState::Starting("Waiting for Tinymist's preview server".to_owned());
        let mut config = TinymistConfig::new(self.project_root())
            .with_executable(self.tinymist_tool.program.clone());
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

    fn receive_tinymist_events(&mut self) {
        while let Some(event) = self.tinymist.try_recv() {
            if !self.document_kind.is_typst()
                || self.settings.preview_preference == PreviewPreference::Native
            {
                // A delayed Stopped event from a deliberately disabled sidecar
                // must not turn the explicit Native choice into a failure.
                continue;
            }
            match event {
                TinymistEvent::Starting { .. } => {
                    self.tinymist_state =
                        ServiceState::Starting("Launching Tinymist LSP".to_owned());
                }
                TinymistEvent::Initialized { .. } => {
                    self.tinymist_state =
                        ServiceState::Starting("Starting Tinymist preview server".to_owned());
                }
                TinymistEvent::PreviewReady { url, .. } => {
                    self.tinymist_url = Some(url);
                    self.tinymist_state =
                        ServiceState::Ready("LSP and preview server are ready".to_owned());
                    self.webview_state =
                        ServiceState::Starting("Embedding the vector preview".to_owned());
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
                TinymistEvent::Error {
                    stage,
                    message,
                    fatal,
                    ..
                } => {
                    let detail = format!("{stage}: {message}");
                    self.tinymist_state = if fatal {
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
            if !self.confirm_before_replacing("following a document link") {
                return;
            }
            self.load_path(path.clone());
            if !self
                .path
                .as_ref()
                .is_some_and(|current| same_path(current, &path))
            {
                return;
            }
        }
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
            if path.extension().is_none_or(|extension| extension != "typ")
                || !self.confirm_before_replacing("following the preview location")
            {
                return;
            }
            self.load_path(path);
        }
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

    fn record_current_fallbacks(&mut self, context: &egui::Context) {
        let mut events = Vec::new();
        if let Some(reason) = self.preview_fallback_reason() {
            events.push(format!("Interactive preview → Native PDF: {reason}"));
        }
        if let Some(reason) = self
            .settings
            .interface_theme
            .fallback_reason(context.system_theme())
        {
            events.push(format!("System appearance → Dark interface: {reason}"));
        }
        if let Some(reason) = &self.typst_tool.fallback_reason {
            events.push(format!("Typst binary: {reason}"));
        }
        if let Some(reason) = &self.tinymist_tool.fallback_reason {
            events.push(format!("Tinymist binary: {reason}"));
        }
        for event in events {
            if !self.fallback_history.contains(&event) {
                self.fallback_history.push(event);
            }
        }
        const MAX_FALLBACK_EVENTS: usize = 8;
        if self.fallback_history.len() > MAX_FALLBACK_EVENTS {
            self.fallback_history
                .drain(..self.fallback_history.len() - MAX_FALLBACK_EVENTS);
        }
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
            let compact = toolbar_width < 430.0;
            if compact {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.spacing_mut().button_padding.x = 4.0;
            }
            if toolbar_width >= 940.0 {
                ui.strong("tiptoptyp");
                ui.separator();
            }

            self.show_file_menu(ui);

            if toolbar_width >= 720.0 {
                if ui
                    .selectable_label(self.problems_visible, "Problems")
                    .on_hover_text("Toggle compiler diagnostics")
                    .clicked()
                {
                    self.problems_visible = !self.problems_visible;
                }
                if ui
                    .selectable_label(self.settings_visible, "Settings")
                    .on_hover_text("Appearance and backend status · ⌘,")
                    .clicked()
                {
                    self.settings_visible = !self.settings_visible;
                }
                if ui.button("Find").on_hover_text("Find · ⌘F").clicked() {
                    self.open_find(false);
                }
            } else {
                ui.menu_button("☰", |ui| {
                    if ui
                        .selectable_label(self.problems_visible, "Problems")
                        .clicked()
                    {
                        self.problems_visible = !self.problems_visible;
                        ui.close();
                    }
                    if ui
                        .selectable_label(self.settings_visible, "Settings")
                        .clicked()
                    {
                        self.settings_visible = !self.settings_visible;
                        ui.close();
                    }
                    if ui.button("Find                 ⌘F").clicked() {
                        self.open_find(false);
                        ui.close();
                    }
                })
                .response
                .on_hover_text("Problems, settings, and find");
            }

            ui.separator();
            let explorer_label = if compact { "▤" } else { "Explorer" };
            if ui
                .selectable_label(self.filesystem_visible, explorer_label)
                .on_hover_text("Toggle the file explorer")
                .clicked()
            {
                self.filesystem_visible = !self.filesystem_visible;
            }
            if compact {
                ui.selectable_value(&mut self.view_mode, ViewMode::Code, "C")
                    .on_hover_text("Code");
                ui.selectable_value(&mut self.view_mode, ViewMode::Split, "S")
                    .on_hover_text("Split");
                ui.selectable_value(&mut self.view_mode, ViewMode::Preview, "P")
                    .on_hover_text("Preview");
            } else {
                ui.selectable_value(&mut self.view_mode, ViewMode::Code, "Code");
                ui.selectable_value(&mut self.view_mode, ViewMode::Split, "Split");
                ui.selectable_value(&mut self.view_mode, ViewMode::Preview, "Preview");
            }

            let title = format!(
                "{}{}",
                self.document_name(),
                if self.is_dirty() { "*" } else { "" }
            );
            if ui.available_width() > 12.0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&title).strong())
                            .truncate()
                            .halign(Align::RIGHT),
                    )
                    .on_hover_text(self.title());
                });
            }
        });
    }

    fn show_file_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File", |ui| {
            if ui.button("New                 ⌘N").clicked() {
                ui.close();
                self.new_document();
            }
            if ui.button("Open…              ⌘O").clicked() {
                ui.close();
                self.open_dialog();
            }
            if ui.button("Save                 ⌘S").clicked() {
                ui.close();
                self.save_document();
            }
            if ui.button("Save As…          ⇧⌘S").clicked() {
                ui.close();
                self.save_as();
            }
            ui.separator();
            if ui.button("Export PDF…").clicked() {
                ui.close();
                self.export_pdf();
            }
        });
    }

    fn show_settings(&mut self, ui: &mut egui::Ui) {
        ui.set_max_width(SETTINGS_PANEL_WIDTH);
        ui.horizontal(|ui| {
            ui.label(RichText::new("SETTINGS").small().strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("×")
                    .on_hover_text("Close Settings")
                    .clicked()
                {
                    self.settings_visible = false;
                }
            });
        });
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
                ui.label("Interface theme");
                ui.horizontal_wrapped(|ui| {
                    for theme in InterfaceTheme::ALL {
                        ui.selectable_value(&mut edited.interface_theme, theme, theme.label());
                    }
                });

                let system_theme = ui.ctx().system_theme();
                let effective_theme = ui.ctx().theme();
                let selected_effective_theme = edited
                    .interface_theme
                    .resolve(system_theme, egui::Theme::Dark);
                egui::Grid::new("appearance-status")
                    .num_columns(2)
                    .spacing(egui::vec2(12.0, 5.0))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Selected").weak());
                        ui.label(edited.interface_theme.label());
                        ui.end_row();
                        ui.label(RichText::new("System").weak());
                        ui.label(theme_label(system_theme));
                        ui.end_row();
                        ui.label(RichText::new("Effective").weak());
                        ui.label(theme_label(Some(effective_theme)));
                        ui.end_row();
                        if selected_effective_theme != effective_theme {
                            ui.label(RichText::new("Next frame").weak());
                            ui.label(theme_label(Some(selected_effective_theme)));
                            ui.end_row();
                        }
                    });
                if let Some(reason) = edited.interface_theme.fallback_reason(system_theme) {
                    fallback_notice(ui, "THEME FALLBACK", reason);
                }

                ui.add_space(10.0);
                ui.label("Document preview appearance");
                ui.horizontal_wrapped(|ui| {
                    for theme in DocumentTheme::ALL {
                        ui.selectable_value(&mut edited.document_theme, theme, theme.label());
                    }
                });
                let effective_document_theme = edited.document_theme.resolve(effective_theme);
                ui.label(
                    RichText::new(format!(
                        "Effective page appearance: {}",
                        theme_label(Some(effective_document_theme))
                    ))
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);
                ui.heading("Editor");
                ui.checkbox(&mut edited.line_wrap, "Wrap long lines");
                ui.checkbox(&mut edited.line_numbers, "Show line numbers");
                ui.horizontal(|ui| {
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
                });
                ui.label(
                    RichText::new(edited.source_preview_trigger.description())
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);
                ui.heading("Tool binaries");
                ui.label(
                    RichText::new(
                        "Packaged builds use pinned sidecars. A custom path overrides one tool without changing the other.",
                    )
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
                tool_preference_editor(ui, "Typst compiler", &mut edited.typst);
                let staged_typst = if edited.typst == self.settings.typst {
                    self.typst_tool.clone()
                } else {
                    resolve_tool(ToolKind::Typst, &edited.typst)
                };
                show_tool_resolution(ui, &staged_typst);
                ui.add_space(8.0);
                tool_preference_editor(ui, "Tinymist language server", &mut edited.tinymist);
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

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);
                ui.heading("Preview backend");
                ui.horizontal_wrapped(|ui| {
                    for preference in PreviewPreference::ALL {
                        ui.selectable_value(
                            &mut edited.preview_preference,
                            preference,
                            preference.label(),
                        );
                    }
                });
                settings_value_row(ui, "Requested", edited.preview_preference.label());
                settings_value_row(ui, "Effective", self.preview_backend_label());
                if let Some(reason) = self.preview_fallback_reason() {
                    fallback_notice(ui, "PREVIEW FALLBACK ACTIVE", &reason);
                } else {
                    ui.label(
                        RichText::new("No preview fallback is active")
                            .small()
                            .color(success_color(ui.visuals().dark_mode)),
                    );
                }
                if self.settings.preview_preference == PreviewPreference::Interactive
                    && !self.interactive_preview_active()
                    && ui.button("Retry Tinymist").clicked()
                {
                    self.restart_tinymist();
                }

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);
                ui.heading("Toolchain status");
                settings_value_row(ui, "Typst executable", &self.typst_tool.detail());
                settings_value_row(ui, "Tinymist executable", &self.tinymist_tool.detail());
                show_service_status(ui, "Tinymist LSP/server", &self.tinymist_state);
                show_service_status(ui, "Embedded vector viewer", &self.webview_state);
                show_service_status(ui, "Typst watcher", &self.compiler_service_state());
                show_service_status(ui, "Native PDF renderer", &self.rasterizer_service_state());
                settings_value_row(ui, "Syntax", "typst-syntax (official parser)");
                settings_value_row(
                    ui,
                    "Project root",
                    &self.project_root().display().to_string(),
                );

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);
                ui.heading("Fallback history");
                ui.label(
                    RichText::new("This session, newest last")
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                if self.fallback_history.is_empty() {
                    ui.label(RichText::new("No recorded fallbacks").weak());
                } else {
                    for event in &self.fallback_history {
                        ui.label(RichText::new(format!("• {event}")).small());
                    }
                }
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
            response.on_hover_text(hover);
            if ui
                .small_button("↻")
                .on_hover_text("Refresh filesystem")
                .clicked()
            {
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
        egui::ScrollArea::both()
            .id_salt("workspace-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(snapshot) = &snapshot {
                    let (_, actions) =
                        TreeView::new(ui.id().with("workspace-tree")).show(ui, |builder| {
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
            && self.path.as_ref() != Some(&path)
            && self.confirm_before_replacing("opening another project file")
        {
            self.load_path(path);
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
            find_previous |= ui
                .small_button("↑")
                .on_hover_text("Previous match · ⇧Enter")
                .clicked();
            find_next |= ui
                .small_button("↓")
                .on_hover_text("Next match · Enter")
                .clicked();
            if ui.small_button("×").on_hover_text("Close · Esc").clicked() {
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
        if replace_one
            && self
                .search
                .replace_one(&mut self.source, &self.find_query, &self.replacement)
        {
            self.pending_editor_selection = self
                .search
                .selected()
                .map(|matched| matched.char_range.clone());
            self.mark_edited();
        }
        if replace_all {
            let count =
                self.search
                    .replace_all(&mut self.source, &self.find_query, &self.replacement);
            if count > 0 {
                self.notice = Some(Notice {
                    message: format!("Replaced {count} matches"),
                    kind: NoticeKind::Success,
                });
                self.mark_edited();
            }
        }
    }

    fn show_editor(&mut self, ui: &mut egui::Ui) {
        let full_path = self.path.as_ref().map_or_else(
            || "Untitled.typ".to_owned(),
            |path| path.display().to_string(),
        );
        show_panel_header(ui, "editor-header", |ui| {
            let width = ui.available_width().max(1.0);
            let job = document_path_layout_job(ui, self.path.as_deref(), self.is_dirty(), width);
            let response = ui.add_sized([width, 20.0], egui::Label::new(job).truncate());
            paint_bold_filename_overlay(ui, response.rect, self.path.as_deref(), self.is_dirty());
            response.on_hover_text(if self.is_dirty() {
                format!("{full_path} (unsaved changes)")
            } else {
                full_path.clone()
            });
        });
        if self.find_visible {
            self.show_find_bar(ui);
            ui.separator();
        }

        let line_diagnostics = self.line_diagnostics();
        let available = ui.available_size();
        let line_count = logical_line_count(&self.source);
        let editor_height = (line_count as f32 * 20.0 + 8.0).max(available.y);
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
        let highlighter = &mut self.highlighter;
        let generic_highlighter = &mut self.generic_highlighter;
        let pending_selection = self.pending_editor_selection.take();
        let attention = self.editor_attention.and_then(|attention| {
            let elapsed = Instant::now().saturating_duration_since(attention.started);
            let strength = editor_attention_strength(elapsed);
            (strength > 0.0).then_some((attention.char_index, strength))
        });
        if attention.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        } else {
            self.editor_attention = None;
        }
        let mut changed = false;
        let mut preview_jump_char = None;

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
                    .id_salt("typst-source")
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
                paint_line_diagnostics(ui, &output, &line_diagnostics, background_slots);
                if line_numbers {
                    paint_line_numbers(ui, &output);
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

        if changed {
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
            if self.update_webview(frame, rect, true) {
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
            if ui
                .selectable_label(true, label)
                .on_hover_text(hover)
                .clicked()
            {
                self.queue_preview_preference(next, ui.ctx());
            }
        } else {
            ui.label(RichText::new("Native").strong())
                .on_hover_text("This file opens directly in the native viewer");
        }

        if !self.interactive_preview_active() {
            let page_count = self.pages.len();
            if header_width >= 185.0 {
                ui.separator();
                if ui
                    .add_enabled(self.visible_page > 0, egui::Button::new("‹"))
                    .on_hover_text("Previous page")
                    .clicked()
                {
                    self.requested_page = Some(self.visible_page - 1);
                }
                ui.label(if page_count == 0 {
                    "–/–".to_owned()
                } else {
                    format!("{}/{page_count}", self.visible_page + 1)
                });
                if ui
                    .add_enabled(self.visible_page + 1 < page_count, egui::Button::new("›"))
                    .on_hover_text("Next page")
                    .clicked()
                {
                    self.requested_page = Some(self.visible_page + 1);
                }
            }
            if header_width >= 315.0 {
                ui.separator();
                if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                    self.requested_zoom =
                        Some((self.zoom / 1.15).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM));
                    self.fit_width = false;
                }
                if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                    self.requested_zoom =
                        Some((self.zoom * 1.15).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM));
                    self.fit_width = false;
                }
                if header_width >= 400.0 {
                    ui.label(format!("{:.0}%", self.zoom * 100.0));
                }
                if ui
                    .selectable_label(self.fit_width, "⇔")
                    .on_hover_text("Fit page width")
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
                        response.on_hover_text(&link.target);
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
            let (status, color) = self.status_label(ui.visuals().dark_mode);
            ui.label(RichText::new(status).small().strong().color(color))
                .on_hover_text(self.status_detail());
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
                    ui.label(
                        RichText::new(format!("⚠ {count}"))
                            .small()
                            .strong()
                            .color(warning_color(ui.visuals().dark_mode)),
                    )
                    .on_hover_text(fallbacks.join("\n"));
                }
                if let Some(notice) = &self.notice {
                    ui.separator();
                    let color = match notice.kind {
                        NoticeKind::Info => info_color(ui.visuals().dark_mode),
                        NoticeKind::Success => success_color(ui.visuals().dark_mode),
                        NoticeKind::Error => error_color(ui.visuals().dark_mode),
                    };
                    ui.add(
                        egui::Label::new(RichText::new(&notice.message).small().color(color))
                            .truncate()
                            .halign(Align::RIGHT),
                    )
                    .on_hover_text(&notice.message);
                }
            });
        });
    }

    fn status_label(&self, dark_mode: bool) -> (String, Color32) {
        match self.status {
            PreviewStatus::Waiting => ("○".to_owned(), neutral_color(dark_mode)),
            PreviewStatus::Compiling => ("↻".to_owned(), info_color(dark_mode)),
            PreviewStatus::Ready(elapsed) => {
                let label = if self.document_kind.is_typst() {
                    format!("✓ {:.0} ms", elapsed.as_secs_f64() * 1000.0)
                } else {
                    "✓".to_owned()
                };
                (label, success_color(dark_mode))
            }
            PreviewStatus::Error => ("⚠".to_owned(), error_color(dark_mode)),
        }
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
    fn update_webview(&mut self, frame: &mut eframe::Frame, rect: Rect, visible: bool) -> bool {
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
            let navigation_root = self.project_root();
            let navigation_dir = self.current_directory();
            let popup_base = url.clone();
            let popup_sender = self.web_link_sender.clone();
            let popup_root = navigation_root.clone();
            let popup_dir = navigation_dir.clone();
            match wry::WebViewBuilder::new()
                .with_url(&url)
                .with_bounds(bounds)
                .with_navigation_handler(move |candidate| {
                    if candidate == "about:blank" {
                        true
                    } else if same_web_origin(&navigation_base, &candidate) {
                        if let Some(target) = preview_document_file_url(
                            &candidate,
                            &navigation_root,
                            navigation_dir.as_deref(),
                        ) {
                            let _ = navigation_sender.send(target);
                            false
                        } else {
                            true
                        }
                    } else {
                        let _ = navigation_sender.send(candidate);
                        false
                    }
                })
                .with_new_window_req_handler(move |candidate, _features| {
                    let target = if same_web_origin(&popup_base, &candidate) {
                        preview_document_file_url(&candidate, &popup_root, popup_dir.as_deref())
                    } else {
                        Some(candidate)
                    };
                    if let Some(target) = target {
                        let _ = popup_sender.send(target);
                    }
                    wry::NewWindowResponse::Deny
                })
                .build_as_child(window.as_ref())
            {
                Ok(webview) => {
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
    fn update_webview(&mut self, _frame: &mut eframe::Frame, _rect: Rect, _visible: bool) -> bool {
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
    fn raw_input_hook(&mut self, context: &egui::Context, _raw_input: &mut egui::RawInput) {
        let Some(settings) = self.pending_settings.take() else {
            return;
        };
        let interface_theme_changed = settings.interface_theme != self.settings.interface_theme;
        self.settings = settings;
        if interface_theme_changed {
            context.set_theme(self.settings.interface_theme.egui_preference());
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
        self.sync_runtime_settings(&context);
        self.receive_compile_results(&context);
        self.receive_asset_results(&context);
        self.poll_export_dialog(&context);
        self.receive_tinymist_events();
        self.receive_web_links();
        self.record_current_fallbacks(&context);
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
        if self.settings_visible {
            self.hide_webview();
            egui::CentralPanel::default()
                .frame(compact_content_panel_frame(ui.style()))
                .show(ui, |ui| self.show_settings(ui));
        } else if self.document_kind.preview_only() {
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

    if let Some((char_index, strength)) = attention {
        let line = line_index_at_char(source, char_index);
        let alpha = (38.0 + 96.0 * strength).round().clamp(0.0, 255.0) as u8;
        let fill = if ui.visuals().dark_mode {
            Color32::from_rgba_unmultiplied(65, 185, 226, alpha)
        } else {
            Color32::from_rgba_unmultiplied(16, 126, 184, alpha.min(105))
        };
        push_editor_line_fill(
            &mut shapes,
            output,
            &line_rows,
            line,
            fill,
            Some(fill.gamma_multiply(1.35)),
        );
    }

    ui.painter().set(slot, egui::Shape::Vec(shapes));
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

fn editor_attention_strength(elapsed: Duration) -> f32 {
    if elapsed >= EDITOR_ATTENTION_DURATION {
        return 0.0;
    }
    let progress = elapsed.as_secs_f32() / EDITOR_ATTENTION_DURATION.as_secs_f32();
    let fade = (1.0 - progress).powi(2);
    let pulse = 0.72 + 0.28 * (progress * std::f32::consts::TAU).sin().abs();
    fade * pulse
}

fn paint_line_diagnostics(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    diagnostics: &[LineDiagnostic],
    slots: Vec<egui::layers::ShapeIdx>,
) {
    let painter = ui.painter();
    let line_rows = logical_line_row_ranges(&output.galley.rows);
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
        let text = format!("  ⟵ {}", diagnostic.summary);
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
        let clip = ui.clip_rect();
        let safe_right = clip.right() - 8.0;
        let tooltip_width = (safe_right - clip.left() - 16.0).clamp(48.0, 440.0);
        let mut tooltip = egui::Tooltip::for_enabled(&response).width(tooltip_width);
        const LEFT_ALTERNATIVES: &[egui::RectAlign] = &[egui::RectAlign::LEFT_END];
        tooltip.popup = tooltip
            .popup
            .anchor(Pos2::new(safe_right, hover_rect.top()))
            .align(egui::RectAlign::LEFT_START)
            .align_alternatives(LEFT_ALTERNATIVES);
        tooltip.show(|ui| {
            ui.label(
                RichText::new(diagnostic.severity.label())
                    .strong()
                    .color(color),
            );
            ui.label(&diagnostic.detail);
        });
    }
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
        let active_marker = if active.is_some_and(|path| same_path(path, &node.path)) {
            " ●"
        } else {
            ""
        };
        if node.is_directory() {
            let open = builder.dir(
                node.path.clone(),
                format!("▾ {}{active_marker}", node.display_name()),
            );
            if open {
                add_workspace_nodes(builder, &node.children, active);
            }
            builder.close_dir();
        } else if node.is_file() {
            let icon = if node
                .path
                .extension()
                .is_some_and(|extension| extension == "typ")
            {
                "T"
            } else {
                "·"
            };
            builder.leaf(
                node.path.clone(),
                format!("{icon} {}{active_marker}", node.display_name()),
            );
        } else if node.is_symlink() {
            builder.leaf(
                node.path.clone(),
                format!("↗ {} (link)", node.display_name()),
            );
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

fn theme_label(theme: Option<egui::Theme>) -> &'static str {
    match theme {
        Some(egui::Theme::Light) => "Light",
        Some(egui::Theme::Dark) => "Dark",
        None => "Unavailable",
    }
}

fn settings_value_row(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{name}:")).weak());
        ui.label(value);
    });
}

fn tool_preference_editor(ui: &mut egui::Ui, label: &str, preference: &mut ToolPreference) {
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
                if ui.button("Browse…").clicked()
                    && let Some(path) = FileDialog::new()
                        .set_title(format!("Choose {label}"))
                        .pick_file()
                {
                    preference.custom_path = path.display().to_string();
                }
            });
        }
    });
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

fn show_service_status(ui: &mut egui::Ui, name: &str, state: &ServiceState) {
    let color = match state {
        ServiceState::Ready(_) => success_color(ui.visuals().dark_mode),
        ServiceState::Starting(_) => info_color(ui.visuals().dark_mode),
        ServiceState::Degraded(_) => warning_color(ui.visuals().dark_mode),
        ServiceState::Failed(_) => error_color(ui.visuals().dark_mode),
        ServiceState::Disabled(_) | ServiceState::Unsupported(_) => {
            neutral_color(ui.visuals().dark_mode)
        }
    };
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(name).strong());
            ui.label(RichText::new(state.label()).small().strong().color(color));
        });
        ui.label(
            RichText::new(state.detail())
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    });
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

fn elided_document_path_parts(path: Option<&Path>, max_chars: usize) -> (String, String) {
    let full = path.map_or_else(
        || "Untitled.typ".to_owned(),
        |path| path.display().to_string(),
    );
    let filename = path
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled.typ".to_owned());
    let prefix = full.strip_suffix(&filename).unwrap_or("");

    if full.chars().count() <= max_chars {
        return (prefix.to_owned(), filename);
    }

    let filename_chars = filename.chars().count();
    if filename_chars >= max_chars {
        return (String::new(), tail_elide(&filename, max_chars));
    }

    let prefix_chars = max_chars - filename_chars;
    (tail_elide(prefix, prefix_chars), filename)
}

fn document_path_layout_job(
    ui: &egui::Ui,
    path: Option<&Path>,
    dirty: bool,
    max_width: f32,
) -> LayoutJob {
    let font_id = egui::TextStyle::Small.resolve(ui.style());
    let max_chars =
        approximate_char_capacity(max_width, font_id.size).saturating_sub(usize::from(dirty));
    let (prefix, filename) = elided_document_path_parts(path, max_chars);
    let mut job = LayoutJob::default();
    job.append(
        &prefix,
        0.0,
        TextFormat {
            font_id: font_id.clone(),
            color: ui.visuals().weak_text_color(),
            ..Default::default()
        },
    );
    job.append(
        &filename,
        0.0,
        TextFormat {
            font_id: font_id.clone(),
            color: ui.visuals().strong_text_color(),
            ..Default::default()
        },
    );
    if dirty {
        job.append(
            "*",
            0.0,
            TextFormat {
                font_id,
                color: warning_color(ui.visuals().dark_mode),
                ..Default::default()
            },
        );
    }
    job
}

fn paint_bold_filename_overlay(ui: &egui::Ui, rect: Rect, path: Option<&Path>, dirty: bool) {
    let font_id = egui::TextStyle::Small.resolve(ui.style());
    let max_chars =
        approximate_char_capacity(rect.width(), font_id.size).saturating_sub(usize::from(dirty));
    let (prefix, filename) = elided_document_path_parts(path, max_chars);
    if filename.is_empty() {
        return;
    }
    let painter = ui.painter().with_clip_rect(rect);
    let prefix_galley =
        painter.layout_no_wrap(prefix, font_id.clone(), ui.visuals().weak_text_color());
    let filename_galley =
        painter.layout_no_wrap(filename, font_id, ui.visuals().strong_text_color());
    let pos = Pos2::new(
        rect.left() + prefix_galley.size().x + 0.55,
        rect.center().y - filename_galley.size().y * 0.5,
    );
    // egui's bundled proportional face has no bold variant. A sub-pixel
    // overprint gives only the filename a genuine heavier stroke while the
    // prefix remains visually quiet.
    painter.galley(pos, filename_galley, ui.visuals().strong_text_color());
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
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
    fn compact_document_path_keeps_the_filename_separate_for_bold_styling() {
        let path = Path::new("/Users/example/a/very/long/project/chapter.typ");
        let (prefix, filename) = elided_document_path_parts(Some(path), 20);
        assert!(prefix.starts_with('…'));
        assert_eq!(filename, "chapter.typ");
        assert_eq!(prefix.chars().count() + filename.chars().count(), 20);
    }

    #[test]
    fn pane_headers_have_equal_exact_and_gapless_geometry() {
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
            ("geometry-editor-header", 1),
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
    fn editor_attention_pulse_fades_out_on_schedule() {
        assert!(editor_attention_strength(Duration::ZERO) > 0.0);
        assert!(
            editor_attention_strength(Duration::from_millis(450))
                < editor_attention_strength(Duration::ZERO)
        );
        assert_eq!(editor_attention_strength(EDITOR_ATTENTION_DURATION), 0.0);
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
