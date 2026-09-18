//! Workspace-level presentation is independent of an open editor document.
use super::*;

pub(super) fn empty_workspace_command(command: AppCommand) -> bool {
    matches!(
        command,
        AppCommand::New
            | AppCommand::NewWindow
            | AppCommand::Open
            | AppCommand::OpenInNewWindow
            | AppCommand::ChangeWorkspaceRoot
            | AppCommand::Settings
            | AppCommand::Explorer
            | AppCommand::Git
            | AppCommand::Packages
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContentView {
    Empty,
    Source,
    Asset,
    SplitSource,
    SplitAsset,
    Preview,
}

pub(super) fn content_view(
    empty: bool,
    kind: DocumentKind,
    typst: bool,
    mode: ViewMode,
) -> ContentView {
    if empty {
        ContentView::Empty
    } else if kind.preview_only() {
        if typst {
            ContentView::SplitAsset
        } else {
            ContentView::Asset
        }
    } else if !typst {
        ContentView::Source
    } else {
        match mode {
            ViewMode::Code => ContentView::Source,
            ViewMode::Split => ContentView::SplitSource,
            ViewMode::Preview => ContentView::Preview,
        }
    }
}

impl EditorApp {
    pub(super) fn status_preview(&self) -> &PreviewController {
        if self.document().kind().preview_only() && !self.typst_preview_available() {
            &self.asset_preview
        } else {
            &self.preview
        }
    }
    pub(super) fn empty_workspace(&mut self, context: &egui::Context) {
        self.stop_tinymist_session();
        self.tabs = self.tabs.empty_after();
        let _ = self.compiler.pause(self.document().revision());
        self.compile_deadline = None;
        self.project_index_deadline.clear();
        self.project_index_job.supersede();
        self.project_index = ProjectIndex::default();
        self.preview.suspend_document("No tab is open");
        self.preview.status = PreviewStatus::Ready(Duration::ZERO);
        self.clear_preview_for_document(false);
        self.hide_webview();
        self.discard_webview();
        self.git_editor.clear_document();
        self.close_app_popup();
        self.search.clear();
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.editor_completion = None;
        self.last_editor_caret = None;
        self.find_visible = false;
        self.replace_visible = false;
        self.document_workflow.revoke_close();
        let editor_id = source_editor_id(context);
        context.memory_mut(|memory| memory.surrender_focus(editor_id));
        self.notice = Some(Notice {
            message: "Workspace ready · no open tabs".into(),
            kind: NoticeKind::Info,
        });
        context.request_repaint();
    }

    pub(super) fn show_empty_workspace(
        &mut self,
        ui: &mut egui::Ui,
        frame: Option<&eframe::Frame>,
    ) {
        ui.add_space((ui.available_height() * 0.4).max(0.0));
        ui.vertical_centered(|ui| {
            ui.heading("No open tabs");
            ui.weak("Open a file from the Explorer, or create a new document.");
            if ui.button("New document").clicked() {
                self.execute_app_command(AppCommand::New, ui.ctx(), frame);
            }
            if ui.button("Open file…").clicked() {
                self.execute_app_command(AppCommand::Open, ui.ctx(), frame);
            }
        });
    }

    pub(super) fn request_asset(&mut self, path: PathBuf) {
        self.asset_preview.status = PreviewStatus::Compiling;
        self.asset_preview.dark =
            self.preview.dark && self.document().kind() != DocumentKind::Image;
        if let Err(error) =
            self.asset_loader
                .request(self.asset_token, path, self.document().kind())
        {
            self.asset_preview.status = PreviewStatus::Error;
            self.notice = Some(Notice {
                message: error,
                kind: NoticeKind::Error,
            });
        }
    }

    pub(super) fn show_asset_view(&mut self, ui: &mut egui::Ui) {
        let fresh = self
            .asset_preview
            .raster_freshness(self.document().revision())
            == Some(RasterContentFreshness::Current);
        let target = ui
            .push_id(
                (
                    "asset-tab",
                    self.tabs
                        .active_id()
                        .expect("asset view requires an active tab"),
                ),
                |ui| {
                    theme::panel_header(ui, "asset-header", |ui| {
                        raster_view::show_controls(ui, &mut self.asset_preview, fresh, false)
                    });
                    if self.asset_preview.content.pages().is_empty() {
                        let failed = self.asset_preview.status == PreviewStatus::Error;
                        show_centered_preview_message(
                            ui,
                            if failed {
                                "Could not load file"
                            } else {
                                "Loading file…"
                            },
                            !failed,
                        );
                        return None;
                    }
                    raster_view::show_pages(ui, &mut self.asset_preview, fresh, false)
                },
            )
            .inner;
        if let Some(target) = target {
            if let Some(page) = internal_pdf_page_target(&target) {
                self.asset_preview.requested_page =
                    Some(page.min(self.asset_preview.content.pages().len().saturating_sub(1)));
            } else {
                self.follow_preview_link_from(&target, self.current_directory());
            }
        }
    }
}
