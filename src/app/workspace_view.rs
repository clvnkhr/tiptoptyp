//! Workspace-level presentation is independent of an open editor document.
use super::*;

pub(super) fn empty_workspace_command(command: AppCommand) -> bool {
    matches!(
        command,
        AppCommand::New
            | AppCommand::NewFromTemplate
            | AppCommand::NewWindow
            | AppCommand::Open
            | AppCommand::OpenInNewWindow
            | AppCommand::ChangeWorkspaceRoot
            | AppCommand::Settings
            | AppCommand::Terminal
            | AppCommand::Panel
            | AppCommand::MaximizePanel
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
    source_preview: bool,
    mode: ViewMode,
    popped_out: bool,
) -> ContentView {
    let source_preview = source_preview && !popped_out;
    if empty {
        ContentView::Empty
    } else if kind.preview_only() {
        if source_preview {
            ContentView::SplitAsset
        } else {
            ContentView::Asset
        }
    } else if !source_preview {
        ContentView::Source
    } else {
        match mode {
            ViewMode::Code => ContentView::Source,
            ViewMode::Split => ContentView::SplitSource,
            ViewMode::Preview => ContentView::Preview,
        }
    }
}

pub(super) fn empty_workspace_copy(workspace_available: bool) -> (&'static str, &'static str) {
    if workspace_available {
        (
            "No open tabs",
            "Open a file from the Explorer, or create a new document.",
        )
    } else {
        (
            "Choose a working directory",
            "The launch directory is unavailable. Choose a folder before creating a document.",
        )
    }
}

impl EditorApp {
    pub(super) fn status_preview(&self) -> &PreviewController {
        if self.document().kind().preview_only() && !self.source_preview_available() {
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
        self.find_bar.search.clear();
        self.explorer.clear_selection();
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.editor_completion = None;
        self.last_editor_caret = None;
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        self.diagnostic_tooltip = None;
        self.clear_editor_hover();
        self.clear_asset_hover();
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
        let workspace_available = self.workspace_root.is_dir();
        let (heading, help) = empty_workspace_copy(workspace_available);
        ui.add_space((ui.available_height() * 0.4).max(0.0));
        ui.vertical_centered(|ui| {
            ui.heading(heading);
            ui.weak(help);
            if workspace_available
                && labeled_empty_action(ui, "New document", "Create a blank document")
            {
                self.execute_app_command(AppCommand::New, ui.ctx(), frame);
            }
            if !workspace_available
                && labeled_empty_action(ui, "Choose folder…", "Choose a working folder")
            {
                self.execute_app_command(AppCommand::ChangeWorkspaceRoot, ui.ctx(), frame);
            }
            if labeled_empty_action(ui, "Open file…", "Open an existing file") {
                self.execute_app_command(AppCommand::Open, ui.ctx(), frame);
            }
        });
    }

    pub(super) fn request_asset(&mut self, path: PathBuf) {
        self.asset_preview.status = PreviewStatus::Compiling;
        self.asset_preview.dark = self.preview.dark
            && (self.document().kind() != DocumentKind::Image || self.settings.dark_images);
        if let Err(error) = self.asset_loader.request(
            self.asset_token,
            path,
            self.document().kind(),
            self.asset_preview.dark,
        ) {
            self.asset_preview.status = PreviewStatus::Error;
            self.notice = Some(Notice {
                message: error,
                kind: NoticeKind::Error,
            });
        }
    }

    pub(super) fn show_asset_view(&mut self, ui: &mut egui::Ui) {
        if self.pdfium_asset_requested() {
            self.show_pdfium_view(ui, true);
            return;
        }
        ui.push_id(
            (
                "asset-tab",
                self.tabs
                    .active_id()
                    .expect("asset view requires an active tab"),
            ),
            |ui| {
                let dark_images = self
                    .pending_settings
                    .as_ref()
                    .unwrap_or(&self.settings)
                    .dark_images;
                let mut appearance = None;
                theme::panel_header(ui, "asset-header", |ui| {
                    appearance =
                        image_view::show_controls(ui, &mut self.asset_preview, dark_images);
                });
                if let Some(dark_images) = appearance {
                    let mut edited = self
                        .pending_settings
                        .clone()
                        .unwrap_or_else(|| self.settings.clone());
                    edited.dark_images = dark_images;
                    self.queue_settings(edited, ui.ctx());
                }
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
                    return;
                }
                image_view::show_pages(ui, &mut self.asset_preview)
            },
        );
    }
}

fn labeled_empty_action(ui: &mut egui::Ui, action: &str, caption: &str) -> bool {
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - 240.0) * 0.5).max(0.0));
        let clicked = crate::app::icons::action_button(ui, action).clicked();
        ui.label(caption);
        clicked
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opted_in_image_assets_follow_dark_page_appearance() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_untitled_kind(DocumentKind::Image);
        app.preview.dark = true;
        app.settings.dark_images = true;
        app.request_asset(directory.path().join("image.png"));
        assert!(
            app.asset_preview.dark,
            "image appearance must follow the selected dark pages when opted in"
        );
    }

    #[test]
    fn image_appearance_refresh_is_once_per_effective_change_and_keeps_stale_image() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut().replace_loaded_unprojected(
            String::new(),
            directory.path().join("image.png"),
            DocumentKind::Image,
            None,
        );
        app.settings.document_theme = crate::settings::DocumentTheme::Dark;
        app.sync_runtime_settings(&context);
        let old_token = app.asset_token;
        app.accept_asset_result(
            &context,
            crate::asset::AssetResult {
                token: old_token,
                output: Ok(crate::asset::LoadedAsset::Image(crate::pdf::PreviewPage {
                    size: [1, 1],
                    rgba: vec![10, 20, 30, 128],
                })),
            },
        );
        let source_key = app.document().key();
        let deadline = app.compile_deadline;
        app.asset_preview.zoom = 2.0;
        app.settings.dark_images = true;
        app.sync_runtime_settings(&context);
        let dark_token = app.asset_token;
        assert_ne!(dark_token, old_token);
        assert!(app.asset_preview.dark && app.asset_preview.has_resident_pages());
        assert_eq!(app.asset_preview.zoom, 2.0);
        assert_eq!(app.asset_preview.status, PreviewStatus::Compiling);
        app.accept_asset_result(
            &context,
            crate::asset::AssetResult {
                token: old_token,
                output: Ok(crate::asset::LoadedAsset::Image(crate::pdf::PreviewPage {
                    size: [1, 1],
                    rgba: vec![255; 4],
                })),
            },
        );
        assert_eq!(
            app.asset_preview.content.pages()[0]
                .resident
                .as_ref()
                .unwrap()
                .rgba
                .as_ref(),
            [10, 20, 30, 128]
        );
        for _ in 0..20 {
            app.sync_runtime_settings(&context);
            assert_eq!(
                app.asset_token, dark_token,
                "idle frames must not schedule another image transform"
            );
        }
        app.settings.document_theme = crate::settings::DocumentTheme::Light;
        app.sync_runtime_settings(&context);
        assert_ne!(app.asset_token, dark_token);
        assert!(!app.asset_preview.dark);
        assert_eq!(app.document().key(), source_key);
        assert_eq!(app.compile_deadline, deadline);
    }

    #[test]
    fn failed_image_appearance_refresh_keeps_pixels_and_ignores_stale_errors() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_untitled_kind(DocumentKind::Image);
        let old = app.asset_token;
        app.accept_asset_result(
            &context,
            crate::asset::AssetResult {
                token: old,
                output: Ok(crate::asset::LoadedAsset::Image(crate::pdf::PreviewPage {
                    size: [1, 1],
                    rgba: vec![10, 20, 30, 128],
                })),
            },
        );
        app.asset_token.advance();
        app.asset_preview.status = PreviewStatus::Compiling;
        app.accept_asset_result(
            &context,
            crate::asset::AssetResult {
                token: old,
                output: Err("stale failure".into()),
            },
        );
        assert_eq!(app.asset_preview.status, PreviewStatus::Compiling);
        app.accept_asset_result(
            &context,
            crate::asset::AssetResult {
                token: app.asset_token,
                output: Err("current failure".into()),
            },
        );
        assert_eq!(app.asset_preview.status, PreviewStatus::Error);
        assert!(app.asset_preview.has_resident_pages());
        assert_eq!(
            app.asset_preview.content.pages()[0]
                .resident
                .as_ref()
                .unwrap()
                .rgba
                .as_ref(),
            [10, 20, 30, 128]
        );
        assert_eq!(app.notice.as_ref().unwrap().message, "current failure");
    }

    #[test]
    fn unavailable_workspaces_get_a_folder_picker_instead_of_new_document_copy() {
        assert_eq!(empty_workspace_copy(true).0, "No open tabs");
        assert_eq!(empty_workspace_copy(false).0, "Choose a working directory");
        assert!(empty_workspace_copy(false).1.contains("unavailable"));
    }

    #[test]
    fn empty_workspace_actions_have_visible_explanatory_labels() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.empty_workspace(&context);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(700.0, 500.0))
            .build_ui_state(
                |ui, app: &mut EditorApp| app.show_empty_workspace(ui, None),
                app,
            );
        harness.run();
        assert!(harness.query_by_label("Create a blank document").is_some());
        assert!(harness.query_by_label("Open an existing file").is_some());
    }

    #[test]
    fn popped_out_preview_has_no_second_renderer_in_main_content() {
        for kind in [
            DocumentKind::Typst,
            DocumentKind::Tex,
            DocumentKind::Text,
            DocumentKind::Image,
            DocumentKind::Pdf,
        ] {
            for mode in [ViewMode::Code, ViewMode::Split, ViewMode::Preview] {
                for source_preview in [false, true] {
                    let main = if kind.preview_only() {
                        ContentView::Asset
                    } else {
                        ContentView::Source
                    };
                    assert_eq!(
                        content_view(false, kind, source_preview, mode, true),
                        main,
                        "popped-out preview rendered a second time in main content ({kind:?}, {mode:?})"
                    );
                    let docked = if !source_preview {
                        main
                    } else if kind.preview_only() {
                        ContentView::SplitAsset
                    } else {
                        match mode {
                            ViewMode::Code => ContentView::Source,
                            ViewMode::Split => ContentView::SplitSource,
                            ViewMode::Preview => ContentView::Preview,
                        }
                    };
                    assert_eq!(
                        content_view(false, kind, source_preview, mode, false),
                        docked
                    );
                    assert_eq!(
                        content_view(true, kind, source_preview, mode, true),
                        ContentView::Empty
                    );
                }
            }
        }
    }
}
