use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align, Color32, ColorImage, FontFamily, FontId, KeyboardShortcut, Layout, Modifiers,
    Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2,
    text::{CCursor, CCursorRange},
};
use egui_ltreeview::{Action as TreeAction, TreeView, TreeViewBuilder};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

use crate::{
    compiler::{CompileRequest, Compiler, PreviewPage},
    diagnostics::{
        Diagnostic, DiagnosticLocation, DiagnosticSeverity, DiagnosticSource,
        parse_typst_short_output,
    },
    highlight::SyntaxHighlighter,
    preview::{
        PAGE_MARGIN, dark_preview_rgba, page_stack_geometry, stack_height, visible_page,
        zoom_anchored_offset,
    },
    search::{SearchState, find, find_all},
    settings::{
        AppSettings, DocumentTheme, InterfaceTheme, PreviewPreference, ToolMode, ToolPreference,
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
const TOOLBAR_HEIGHT: f32 = 44.0;
const STATUS_HEIGHT: f32 = 27.0;
const PREVIEW_HEADER_HEIGHT: f32 = 43.0;
const SETTINGS_PANEL_WIDTH: f32 = 390.0;
const MIN_PREVIEW_ZOOM: f32 = 0.2;
const MAX_PREVIEW_ZOOM: f32 = 6.0;

const DEFAULT_SOURCE: &str = r##"#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")

= Welcome to mytypst

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
    size: [usize; 2],
    rgba: Vec<u8>,
    texture: TextureHandle,
}

#[derive(Clone)]
struct LineDiagnostic {
    line: usize,
    severity: DiagnosticSeverity,
    summary: String,
    detail: String,
}

pub struct EditorApp {
    source: String,
    path: Option<PathBuf>,
    revision: u64,
    saved_revision: u64,
    disk_fingerprint: Option<u64>,
    highlighter: SyntaxHighlighter,

    compiler: Compiler,
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

    pending_export: Option<PathBuf>,
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

        let mut app = Self {
            source: DEFAULT_SOURCE.to_owned(),
            path: None,
            revision: 0,
            saved_revision: 0,
            disk_fingerprint: None,
            highlighter: SyntaxHighlighter::default(),
            compiler: Compiler::new(context.egui_ctx.clone()),
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
            pending_export: None,
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
        };

        if let Some(path) = initial_path {
            app.load_path(path);
        } else {
            app.reset_document_services();
        }
        app
    }

    fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
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
        format!("{}{dirty} — mytypst", self.document_name())
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
        self.compile_deadline = Some(Instant::now() + COMPILE_DEBOUNCE);
        self.status = PreviewStatus::Waiting;
        self.notice = None;
        self.sync_tinymist_change();
    }

    fn request_compile(&mut self) {
        self.compile_deadline = None;
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
            if result.revision != self.revision {
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
        self.compile_deadline = Some(Instant::now());
        self.status = PreviewStatus::Waiting;
    }

    fn clear_preview_for_document(&mut self) {
        self.compiled_revision = None;
        self.pdf = None;
        self.pages.clear();
        self.visible_page = 0;
        self.raw_diagnostics.clear();
        self.diagnostics.clear();
        self.tinymist_diagnostics.clear();
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
                .find(|path| {
                    !path.as_os_str().is_empty()
                        && path.extension().is_some_and(|extension| extension == "typ")
                })
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
            if self.confirm_before_replacing("closing mytypst") {
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
            .add_filter("Typst documents", &["typ"])
            .set_title("Open Typst document");
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        if let Some(path) = dialog.pick_file() {
            self.load_path(path);
        }
    }

    fn load_path(&mut self, path: PathBuf) {
        match fs::read_to_string(&path) {
            Ok(source) => {
                self.disk_fingerprint = Some(fingerprint(source.as_bytes()));
                self.source = source;
                self.path = Some(path.canonicalize().unwrap_or(path));
                self.revision = self.revision.wrapping_add(1);
                self.saved_revision = self.revision;
                self.clear_preview_for_document();
                self.search.clear();
                self.reset_document_services();
                self.schedule_compile_now();
            }
            Err(error) => {
                self.show_file_error(format!("Could not open {}: {error}", path.display()))
            }
        }
    }

    fn save_document(&mut self) -> bool {
        if let Some(path) = self.path.clone() {
            self.save_to(path)
        } else {
            self.save_as()
        }
    }

    fn save_as(&mut self) -> bool {
        let mut dialog = FileDialog::new()
            .add_filter("Typst documents", &["typ"])
            .set_title("Save Typst document")
            .set_file_name(self.document_name());
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        let Some(mut path) = dialog.save_file() else {
            return false;
        };
        if path.extension().is_none() {
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
                    "{} was deleted outside mytypst. Create it again?",
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
        let default_name = self
            .path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| format!("{}.pdf", stem.to_string_lossy()))
            .unwrap_or_else(|| "document.pdf".to_owned());
        let mut dialog = FileDialog::new()
            .add_filter("PDF documents", &["pdf"])
            .set_title("Export PDF")
            .set_file_name(default_name);
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        let Some(mut path) = dialog.save_file() else {
            return;
        };
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

        self.pending_export = Some(path);
        self.notice = Some(Notice {
            message: "Export queued for the next successful build".to_owned(),
            kind: NoticeKind::Info,
        });
        self.schedule_compile_now();
    }

    fn complete_pending_export(&mut self) {
        let (Some(path), Some(pdf)) = (self.pending_export.take(), self.pdf.as_deref()) else {
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
            .set_title("mytypst")
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
            if self.settings.preview_preference == PreviewPreference::Native {
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
            self.pending_editor_selection = Some(lsp_range_to_char_range(&self.source, selection));
        }
        self.view_mode = ViewMode::Split;
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
        self.settings.preview_preference == PreviewPreference::Interactive
            && self.tinymist_url.is_some()
            && self.webview_state.is_ready()
            && cfg!(any(target_os = "macos", target_os = "windows"))
    }

    fn should_attempt_interactive_preview(&self) -> bool {
        self.settings.preview_preference == PreviewPreference::Interactive
            && self.tinymist_url.is_some()
            && !matches!(
                self.webview_state,
                ServiceState::Failed(_) | ServiceState::Unsupported(_)
            )
            && cfg!(any(target_os = "macos", target_os = "windows"))
    }

    fn preview_fallback_reason(&self) -> Option<String> {
        preview_fallback_reason_for(
            self.settings.preview_preference,
            self.interactive_preview_active(),
            self.tinymist_url.is_some(),
            &self.tinymist_state,
            &self.webview_state,
        )
    }

    fn preview_backend_label(&self) -> &'static str {
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
        for (index, page) in self.pages.iter_mut().enumerate() {
            let pixels = if self.preview_dark {
                dark_preview_rgba(&page.rgba)
            } else {
                page.rgba.clone()
            };
            page.texture = context.load_texture(
                format!("preview-{revision}-{index}-{}", self.preview_dark),
                ColorImage::from_rgba_unmultiplied(page.size, &pixels),
                TextureOptions::LINEAR,
            );
        }
    }

    fn show_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("mytypst");
            ui.separator();
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
            if ui
                .selectable_label(self.filesystem_visible, "Explorer")
                .on_hover_text("Toggle the project filesystem")
                .clicked()
            {
                self.filesystem_visible = !self.filesystem_visible;
            }
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

            ui.separator();
            ui.selectable_value(&mut self.view_mode, ViewMode::Code, "Code");
            ui.selectable_value(&mut self.view_mode, ViewMode::Split, "Split");
            ui.selectable_value(&mut self.view_mode, ViewMode::Preview, "Preview");
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
                ui.label(
                    RichText::new("Both are enabled by default and apply immediately.")
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
        ui.horizontal(|ui| {
            ui.label(RichText::new("PROJECT").small().strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("↻")
                    .on_hover_text("Refresh filesystem")
                    .clicked()
                {
                    self.refresh_workspace();
                }
            });
        });
        if let Some(workspace) = &self.workspace {
            ui.label(
                RichText::new(workspace.root().display().to_string())
                    .small()
                    .color(ui.visuals().weak_text_color()),
            )
            .on_hover_text(format!(
                "Filesystem snapshot generation {}",
                workspace.generation()
            ));
        }
        ui.separator();

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
                                path.extension().is_some_and(|extension| extension == "typ")
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
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.document_name()).strong());
            if self.is_dirty() {
                ui.label(
                    RichText::new("modified")
                        .small()
                        .color(warning_color(ui.visuals().dark_mode)),
                );
            }
        });
        ui.label(
            RichText::new(self.path.as_ref().map_or_else(
                || "Unsaved · relative imports use the project folder".to_owned(),
                |path| path.display().to_string(),
            ))
            .small()
            .color(ui.visuals().weak_text_color()),
        );
        if self.find_visible {
            ui.separator();
            self.show_find_bar(ui);
        }
        ui.separator();

        let line_diagnostics = self.line_diagnostics();
        let available = ui.available_size();
        let line_count = logical_line_count(&self.source);
        let editor_height = (line_count as f32 * 20.0 + 32.0).max(available.y - 4.0);
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
        let highlighter = &mut self.highlighter;
        let pending_selection = self.pending_editor_selection.take();
        let mut changed = false;

        egui::ScrollArea::new([!line_wrap, true])
            .id_salt("source-editor-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let editor_width = if line_wrap {
                    ui.available_width().max(24.0)
                } else {
                    unwrapped_editor_width
                };
                let background_slots = line_diagnostics
                    .iter()
                    .map(|_| ui.painter().add(egui::Shape::Noop))
                    .collect::<Vec<_>>();
                let mut layouter =
                    |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                        let mut job = highlighter.highlight(buffer.as_str(), dark_mode);
                        job.wrap.max_width = if line_wrap { wrap_width } else { f32::INFINITY };
                        ui.fonts_mut(|fonts| fonts.layout_job(job))
                    };
                let editor = egui::TextEdit::multiline(&mut self.source)
                    .id_salt("typst-source")
                    .code_editor()
                    .desired_width(editor_width)
                    .margin(egui::Margin {
                        left: gutter_width,
                        right: 4,
                        top: 2,
                        bottom: 2,
                    })
                    .layouter(&mut layouter);
                let mut output = editor.show(ui);
                changed = output.response.changed();

                paint_line_diagnostics(ui, &output, &line_diagnostics, background_slots);
                if line_numbers {
                    paint_line_numbers(ui, &output);
                }

                if let Some(range) = pending_selection {
                    let cursor_range =
                        CCursorRange::two(CCursor::new(range.start), CCursor::new(range.end));
                    output.state.cursor.set_char_range(Some(cursor_range));
                    output.state.store(ui.ctx(), output.response.id);
                    output.response.request_focus();
                    let cursor_rect = output
                        .galley
                        .pos_from_cursor(CCursor::new(range.start))
                        .translate(output.galley_pos.to_vec2());
                    ui.scroll_to_rect(cursor_rect, Some(Align::Center));
                }

                let used = output.response.rect.size();
                ui.allocate_space(Vec2::new(
                    (editor_width - used.x).max(0.0),
                    (editor_height - used.y).max(0.0),
                ));
            });

        if changed {
            self.search.clear();
            self.mark_edited();
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
        let header = ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), PREVIEW_HEADER_HEIGHT),
            Layout::left_to_right(Align::Center),
            |ui| self.show_preview_header_controls(ui),
        );
        ui.painter().line_segment(
            [
                header.response.rect.left_bottom(),
                header.response.rect.right_bottom(),
            ],
            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );

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
        ui.label(RichText::new("Preview").strong());
        ui.separator();
        if ui
            .selectable_label(
                self.settings.preview_preference == PreviewPreference::Interactive,
                "Interactive",
            )
            .on_hover_text("Tinymist vector preview with hover and source navigation")
            .clicked()
        {
            self.queue_preview_preference(PreviewPreference::Interactive, ui.ctx());
        }
        if ui
            .selectable_label(
                self.settings.preview_preference == PreviewPreference::Native,
                "Native",
            )
            .on_hover_text("Raster recovery viewer using the canonical watched PDF")
            .clicked()
        {
            self.queue_preview_preference(PreviewPreference::Native, ui.ctx());
        }

        if !self.interactive_preview_active() {
            ui.separator();
            let page_count = self.pages.len();
            if ui
                .add_enabled(self.visible_page > 0, egui::Button::new("‹"))
                .on_hover_text("Previous page")
                .clicked()
            {
                self.requested_page = Some(self.visible_page - 1);
            }
            ui.label(if page_count == 0 {
                "– / –".to_owned()
            } else {
                format!("{} / {page_count}", self.visible_page + 1)
            });
            if ui
                .add_enabled(self.visible_page + 1 < page_count, egui::Button::new("›"))
                .on_hover_text("Next page")
                .clicked()
            {
                self.requested_page = Some(self.visible_page + 1);
            }
            ui.separator();
            if ui.small_button("−").clicked() {
                self.requested_zoom =
                    Some((self.zoom / 1.15).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM));
                self.fit_width = false;
            }
            if ui.small_button("+").clicked() {
                self.requested_zoom =
                    Some((self.zoom * 1.15).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM));
                self.fit_width = false;
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0));
            ui.checkbox(&mut self.fit_width, "Fit");
        }

        let mut dark_page = self.preview_dark;
        if ui.checkbox(&mut dark_page, "Dark page").changed() {
            let mut settings = self
                .pending_settings
                .clone()
                .unwrap_or_else(|| self.settings.clone());
            settings.document_theme = if dark_page {
                DocumentTheme::Dark
            } else {
                DocumentTheme::Light
            };
            self.queue_settings(settings, ui.ctx());
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
        let non_preview_fallbacks = self.non_preview_fallback_details(ui.ctx().system_theme());
        ui.horizontal(|ui| {
            let (status, color) = self.status_label(ui.visuals().dark_mode);
            ui.label(RichText::new(status).small().color(color));
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "{} lines · {} chars",
                    self.source.lines().count().max(1),
                    self.source.chars().count()
                ))
                .small(),
            );
            if let Some(notice) = &self.notice {
                ui.separator();
                let color = match notice.kind {
                    NoticeKind::Info => info_color(ui.visuals().dark_mode),
                    NoticeKind::Success => success_color(ui.visuals().dark_mode),
                    NoticeKind::Error => error_color(ui.visuals().dark_mode),
                };
                ui.label(RichText::new(&notice.message).small().color(color));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let fallback = self.preview_fallback_reason();
                let color = if fallback.is_some() {
                    warning_color(ui.visuals().dark_mode)
                } else {
                    ui.visuals().strong_text_color()
                };
                let response = ui.label(
                    RichText::new(self.preview_backend_label())
                        .small()
                        .strong()
                        .color(color),
                );
                if let Some(reason) = fallback {
                    response.on_hover_text(reason);
                }
                if !non_preview_fallbacks.is_empty() {
                    ui.separator();
                    let count = non_preview_fallbacks.len();
                    ui.label(
                        RichText::new(format!(
                            "⚠ {count} fallback{}",
                            if count == 1 { "" } else { "s" }
                        ))
                        .small()
                        .strong()
                        .color(warning_color(ui.visuals().dark_mode)),
                    )
                    .on_hover_text(non_preview_fallbacks.join("\n"));
                }
            });
        });
    }

    fn status_label(&self, dark_mode: bool) -> (String, Color32) {
        match self.status {
            PreviewStatus::Waiting => ("PDF build queued…".to_owned(), neutral_color(dark_mode)),
            PreviewStatus::Compiling => ("PDF compiling…".to_owned(), info_color(dark_mode)),
            PreviewStatus::Ready(elapsed) => (
                format!("PDF ready in {:.0} ms", elapsed.as_secs_f64() * 1000.0),
                success_color(dark_mode),
            ),
            PreviewStatus::Error => ("PDF build error".to_owned(), error_color(dark_mode)),
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
            match wry::WebViewBuilder::new()
                .with_url(&url)
                .with_bounds(bounds)
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
        self.receive_tinymist_events();
        self.record_current_fallbacks(&context);
        self.handle_shortcuts(&context);
        self.handle_dropped_file(&context);
        self.handle_close_request(&context);
        self.update_title(&context);
        self.tick_workspace(&context);

        egui::Panel::top("toolbar")
            .exact_size(TOOLBAR_HEIGHT)
            .show(ui, |ui| self.show_toolbar(ui));
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
                .resizable(true)
                .default_size(230.0)
                .min_size(150.0)
                .max_size(420.0)
                .show(ui, |ui| self.show_workspace(ui));
        }
        if self.settings_visible {
            self.hide_webview();
            egui::CentralPanel::default().show(ui, |ui| self.show_settings(ui));
        } else {
            match self.view_mode {
                ViewMode::Code => {
                    self.hide_webview();
                    egui::CentralPanel::default().show(ui, |ui| self.show_editor(ui));
                }
                ViewMode::Split => {
                    let editor_width = (ui.available_width() * 0.52).max(320.0);
                    egui::Panel::left("editor")
                        .resizable(true)
                        .default_size(editor_width)
                        .min_size(280.0)
                        .show(ui, |ui| self.show_editor(ui));
                    egui::CentralPanel::default().show(ui, |ui| self.show_preview(ui, frame));
                }
                ViewMode::Preview => {
                    egui::CentralPanel::default().show(ui, |ui| self.show_preview(ui, frame));
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
    let pixels = if dark {
        dark_preview_rgba(&page.rgba)
    } else {
        page.rgba.clone()
    };
    let texture = context.load_texture(
        format!("preview-{revision}-{index}-{dark}"),
        ColorImage::from_rgba_unmultiplied(page.size, &pixels),
        TextureOptions::LINEAR,
    );
    PreviewTexture {
        size: page.size,
        rgba: page.rgba,
        texture,
    }
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
                color.gamma_multiply(0.11),
            ));
            background.push(egui::Shape::line_segment(
                [line_rect.left_bottom(), line_rect.right_bottom()],
                Stroke::new(1.0, color.gamma_multiply(0.8)),
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
        ui.interact(
            hover_rect,
            output.response.id.with(("diagnostic", diagnostic.line)),
            Sense::hover(),
        )
        .on_hover_ui_at_pointer(|ui| {
            ui.set_max_width(440.0);
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
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(9.0, 4.0);
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            FontId::new(15.0, FontFamily::Monospace),
        );
    });
}

fn preview_background(ui: &egui::Ui) -> Color32 {
    ui.visuals().panel_fill
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
    fn revision_versions_saturate_for_lsp() {
        assert_eq!(revision_as_i32(42), 42);
        assert_eq!(revision_as_i32(u64::MAX), i32::MAX);
    }

    #[test]
    fn logical_line_count_preserves_empty_and_trailing_lines() {
        assert_eq!(logical_line_count(""), 1);
        assert_eq!(logical_line_count("a\n"), 2);
        assert_eq!(logical_line_count("a\n\n🙂"), 3);
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
        assert_eq!(dark.spacing.item_spacing, egui::vec2(8.0, 7.0));
        assert_eq!(dark.spacing.button_padding, egui::vec2(9.0, 4.0));
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
