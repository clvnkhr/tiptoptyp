//! Activity is observed, never driven, by this panel. Opening it starts no jobs.
use super::*;
use crate::activity::Activity;

#[derive(Default)]
pub(super) struct ActivityTracking {
    pub(super) completion: Option<u64>,
    pub(super) hover: Option<u64>,
    pub(super) intelligence_error: Option<String>,
    pub(super) diagnostics: [Option<DocumentKey>; 3],
    pub(super) build: Activity,
    pub(super) index_error: Option<String>,
    pub(super) format_error: Option<String>,
    pub(super) save_error: Option<String>,
    pub(super) watcher: Activity,
    pub(super) workspace: Activity,
}

fn service(state: &ServiceState) -> Activity {
    match state {
        ServiceState::Ready(_) => Activity::Idle,
        ServiceState::Starting(_) => Activity::Running,
        ServiceState::Failed(e) | ServiceState::Degraded(e) => Activity::Failed(e.clone()),
        ServiceState::Disabled(_) => Activity::Inactive("Disabled"),
        ServiceState::Unsupported(_) => Activity::Inactive("Unavailable"),
    }
}
fn freshness(completed: Option<DocumentKey>, current: DocumentKey) -> Activity {
    if completed == Some(current) {
        Activity::Idle
    } else {
        Activity::Pending("Stale")
    }
}
fn queued(state: Activity, pending: bool, reason: &'static str) -> Activity {
    if !matches!(state, Activity::Running) && pending {
        Activity::Pending(reason)
    } else {
        state
    }
}
fn preview_work(status: PreviewStatus) -> Activity {
    match status {
        PreviewStatus::Waiting => Activity::Pending("Queued"),
        PreviewStatus::Compiling => Activity::Running,
        PreviewStatus::Ready(_) => Activity::Idle,
        PreviewStatus::Error => Activity::Failed("Could not produce preview; see Problems".into()),
    }
}
struct Indicator {
    name: &'static str,
    role: &'static str,
    state: Activity,
}
impl EditorApp {
    fn activity_indicators(&mut self) -> Vec<Indicator> {
        let key = self.document().key();
        let typst = self.document().kind() == DocumentKind::Typst;
        let tex = self.document().kind() == DocumentKind::Tex;
        let inactive = Activity::Inactive("Not in use");
        let mut rows = Vec::with_capacity(28);
        let mut add = |name, role, state| rows.push(Indicator { name, role, state });
        let save_state = if self.save_job.is_running() {
            Activity::Running
        } else {
            self.activity
                .save_error
                .as_ref()
                .map_or_else(|| self.save_job.activity(), |e| Activity::Failed(e.clone()))
        };
        let save = queued(
            save_state,
            self.active_autosave_deadline().is_some(),
            "Save queued",
        );
        add(
            "Save",
            "Writes documents to disk. Autosave waits for edits to settle.",
            if matches!(save, Activity::Idle) && !self.settings.auto_save {
                Activity::Inactive("Autosave off")
            } else if matches!(save, Activity::Idle) && self.document().path().is_none() {
                Activity::Inactive("Choose a path first")
            } else {
                save
            },
        );
        let build = queued(
            self.activity.build.clone(),
            self.compile_deadline.is_some(),
            "Build queued",
        );
        add(
            "Typst",
            "Builds a PDF from your Typst document.",
            if self.preview_document_kind() == DocumentKind::Typst {
                build.clone()
            } else {
                inactive.clone()
            },
        );
        add(
            "Tectonic",
            "Builds a PDF from your TeX document.",
            if self.preview_document_kind() == DocumentKind::Tex && self.settings.tex.build_enabled
            {
                build
            } else {
                inactive.clone()
            },
        );
        let intelligence_busy = self.activity.completion.is_some() || self.activity.hover.is_some();
        let tinymist = if typst {
            service(&self.preview.tinymist_state)
        } else {
            inactive.clone()
        };
        add(
            "Tinymist",
            "Provides Typst suggestions, hover help, and error checks.",
            if matches!(tinymist, Activity::Idle) {
                if intelligence_busy || self.format_request_key.is_some() {
                    Activity::Running
                } else if let Some(error) = &self.activity.intelligence_error {
                    Activity::Failed(error.clone())
                } else {
                    freshness(self.activity.diagnostics[0], key)
                }
            } else {
                tinymist.clone()
            },
        );
        add(
            "Live preview",
            "Updates the live document preview as you type.",
            if typst && self.preview.tinymist_preview_enabled {
                if self.compilation_paused {
                    Activity::Inactive("Paused")
                } else {
                    preview_work(self.preview.status)
                }
            } else {
                inactive.clone()
            },
        );
        add(
            "Typst diagnostics",
            "Checks your Typst document for errors and warnings.",
            if matches!(tinymist, Activity::Idle) {
                freshness(self.activity.diagnostics[0], key)
            } else {
                tinymist
            },
        );
        for (index, name, enabled, ready, error) in [
            (
                1,
                "TexLab",
                self.settings.tex.texlab_enabled,
                self.tex_service.texlab_ready,
                self.tex_service.texlab_error.as_deref(),
            ),
            (
                2,
                "Badness",
                self.settings.tex.needs_badness(),
                self.tex_service.badness_ready,
                self.tex_service.badness_error.as_deref(),
            ),
        ] {
            let checks = if index == 1 {
                self.settings.tex.diagnostics
            } else {
                self.settings.tex.lint
            };
            add(
                name,
                if index == 1 {
                    "Provides TeX suggestions, hover help, and error checks."
                } else {
                    "Checks TeX style and tidies its formatting."
                },
                if !tex || !enabled {
                    inactive.clone()
                } else if let Some(error) = error {
                    Activity::Failed(error.into())
                } else if !ready
                    || (index == 1 && intelligence_busy)
                    || (index == 2
                        && self.format_request_key.is_some()
                        && self.settings.tex.formatter == crate::tex::settings::Formatter::Badness)
                {
                    Activity::Running
                } else if index == 1 && self.activity.intelligence_error.is_some() {
                    Activity::Failed(self.activity.intelligence_error.clone().unwrap())
                } else if checks {
                    freshness(self.activity.diagnostics[index], key)
                } else {
                    Activity::Idle
                },
            );
        }
        let formatter = if tex {
            match self.settings.tex.formatter {
                crate::tex::settings::Formatter::Badness => "Badness format",
                crate::tex::settings::Formatter::TexFmt => "tex-fmt",
                crate::tex::settings::Formatter::Disabled => "Formatting",
            }
        } else {
            "Tinymist format"
        };
        add(
            formatter,
            "Tidies source layout when you format or save.",
            if !typst && !tex
                || tex && self.settings.tex.formatter == crate::tex::settings::Formatter::Disabled
            {
                inactive.clone()
            } else {
                Activity::work(
                    self.format_request_key.is_some(),
                    self.format_when_service_ready.is_some(),
                    self.activity.format_error.as_deref(),
                )
            },
        );
        add(
            "Compiler diagnostics",
            "Collects errors and warnings from PDF builds.",
            if self
                .preview_document_kind()
                .typesetting_language()
                .is_some()
            {
                queued(
                    self.activity.build.clone(),
                    self.compile_deadline.is_some(),
                    "Stale",
                )
            } else {
                inactive.clone()
            },
        );
        add(
            "Renderer",
            "Displays the live preview or PDF in the preview pane.",
            service(&self.preview.webview_state),
        );
        add(
            "PDFium",
            "Draws PDF pages and searches their text when PDFium is selected.",
            self.pdfium_preview.activity(),
        );
        add(
            "PDF viewer",
            "Draws opened PDF files and searches their text.",
            self.pdfium_asset.activity(),
        );
        add(
            "Asset loading",
            "Opens PDFs and images for viewing.",
            if self.document().kind().preview_only() {
                preview_work(self.asset_preview.status)
            } else {
                inactive.clone()
            },
        );
        add(
            "Hover preview",
            "Shows a small image or PDF preview when you hover.",
            self.hover_activity(),
        );
        add(
            "Git status",
            "Checks changed files and handles Git repository actions.",
            self.git.activity(),
        );
        add(
            "Git hunks",
            "Marks changed lines beside your source code.",
            self.git_editor.activity(),
        );
        add(
            "Git mutation",
            "Stages, unstages, or discards selected changes.",
            self.git_hunk_job.activity(),
        );
        add(
            "Workspace",
            "Refreshes the workspace file list when files change.",
            if self.workspace_service.is_subscribed() {
                self.activity.workspace.clone()
            } else {
                inactive.clone()
            },
        );
        add(
            "File watcher",
            "Notices files changed outside the app.",
            if self.workspace_service.is_subscribed() {
                self.activity.watcher.clone()
            } else {
                inactive.clone()
            },
        );
        add(
            "Project index",
            "Finds headings, labels, and references across your project.",
            Activity::work(
                self.project_index_job.is_running(),
                self.project_index_deadline.is_pending(),
                self.activity.index_error.as_deref(),
            ),
        );
        add(
            "Harper",
            "English spelling and grammar checks.",
            if self.settings.english_grammar {
                self.writing.activity()
            } else {
                inactive.clone()
            },
        );
        add(
            "Unicode",
            "Flags invisible or easily confused characters.",
            if self.settings.unicode_warnings {
                self.writing.activity()
            } else {
                inactive.clone()
            },
        );
        add(
            "Fonts",
            "Finds fonts available in your workspace.",
            self.font_catalog_scan.activity(),
        );
        add(
            "Packages",
            "Loads the list of available Typst packages.",
            if self.package_catalog_job.is_running() {
                Activity::Running
            } else if let Some(error) = self
                .package_catalog
                .as_ref()
                .and_then(|c| c.official_index_error.as_ref())
            {
                Activity::Failed(error.to_string())
            } else {
                self.package_catalog_job.activity()
            },
        );
        add(
            "Package removal",
            "Removes installed packages.",
            self.package_uninstall.activity(),
        );
        add(
            "File import",
            "Imports dropped files into the workspace.",
            queued(
                self.file_import.activity(),
                !self.queued_file_drops.is_empty(),
                "Queued",
            ),
        );
        add(
            "Terminal",
            "Runs your shell. Green means connected; commands may still be running.",
            self.terminal.activity(),
        );
        rows
    }
    pub(super) fn show_activity(&mut self, ui: &mut egui::Ui) {
        let rows = if self.snapshot_scene == Some(UiSnapshotScene::ActivityPanel) {
            fixture_indicators()
        } else {
            self.activity_indicators()
        };
        show_indicators(ui, &rows);
    }
    fn hover_activity(&self) -> Activity {
        match self.asset_hover.as_ref().map(|h| &h.content) {
            None => Activity::Inactive("Not in use"),
            Some(AssetHoverContent::Error(error)) => Activity::Failed(error.clone()),
            Some(AssetHoverContent::Loading) => Activity::Running,
            _ => Activity::Idle,
        }
    }
}
fn show_indicators(ui: &mut egui::Ui, rows: &[Indicator]) {
    egui::ScrollArea::vertical()
        .id_salt("activity-scroll")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(5.0, 4.0);
            ui.horizontal_wrapped(|ui| {
                for row in rows {
                    let palette = theme::palette(ui.ctx());
                    let color = match row.state {
                        Activity::Idle => palette.success,
                        Activity::Running | Activity::Pending(_) => palette.warning,
                        Activity::Failed(_) => palette.error,
                        Activity::Inactive(_) => ui.visuals().weak_text_color(),
                    };
                    let response = ui.add(
                        egui::Label::new(
                            RichText::new(format!("● {}", row.name))
                                .color(color)
                                .size(theme::TYPE.supporting),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                    let detail = match &row.state {
                        Activity::Failed(error) => {
                            format!("{}\n{}\n{error}", row.role, row.state.label())
                        }
                        _ => format!("{}\n{}", row.role, row.state.label()),
                    };
                    native_hover_text(response, detail);
                }
            });
        });
}

fn fixture_indicators() -> Vec<Indicator> {
    [
        ("Save", Activity::Pending("Save queued")),
        ("Typst", Activity::Running),
        ("Tectonic", Activity::Inactive("Not in use")),
        ("Tinymist", Activity::Idle),
        ("Live preview", Activity::Running),
        ("Typst diagnostics", Activity::Pending("Stale")),
        ("TexLab", Activity::Inactive("Not in use")),
        ("Badness", Activity::Inactive("Not in use")),
        ("Tinymist format", Activity::Idle),
        ("Compiler diagnostics", Activity::Pending("Stale")),
        ("Renderer", Activity::Idle),
        ("PDFium", Activity::Inactive("Not in use")),
        ("PDF viewer", Activity::Inactive("Not in use")),
        ("Asset loading", Activity::Idle),
        ("Hover preview", Activity::Inactive("Not in use")),
        (
            "Git status",
            Activity::Failed("Example: Git executable unavailable".into()),
        ),
        ("Git hunks", Activity::Pending("Stale")),
        ("Git mutation", Activity::Idle),
        ("Workspace", Activity::Idle),
        ("File watcher", Activity::Idle),
        ("Project index", Activity::Running),
        ("Harper", Activity::Pending("Stale")),
        ("Unicode", Activity::Pending("Stale")),
        ("Fonts", Activity::Idle),
        ("Packages", Activity::Idle),
        ("Package removal", Activity::Idle),
        ("File import", Activity::Idle),
        ("Terminal", Activity::Idle),
    ]
    .into_iter()
    .map(|(name, state)| Indicator {
        name,
        role: "Deterministic QA example",
        state,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn compact_chips_wrap_without_horizontal_overflow() {
        for width in [320.0, 800.0, 1400.0] {
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width, 250.0))
                .build_ui(|ui| show_indicators(ui, &fixture_indicators()));
            harness.run();
            let save = harness.get_by_label("● Save").rect();
            let typst = harness.get_by_label("● Typst").rect();
            assert_eq!(save.min.y, typst.min.y, "indicators should share rows");
            for row in fixture_indicators() {
                let rect = harness.get_by_label(&format!("● {}", row.name)).rect();
                assert!(
                    rect.min.x >= 0.0 && rect.max.x <= width,
                    "{rect:?} at width {width}"
                );
            }
            if width >= 800.0 {
                assert!(harness.get_by_label("● Terminal").rect().max.y < 160.0);
            }
        }
    }
    #[test]
    fn activity_state_changes_cannot_move_any_label() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(800.0, 250.0))
            .build_ui_state(|ui, rows| show_indicators(ui, rows), fixture_indicators());
        harness.run();
        let before: Vec<_> = harness
            .state()
            .iter()
            .map(|row| harness.get_by_label(&format!("● {}", row.name)).rect())
            .collect();
        for state in [
            Activity::Idle,
            Activity::Running,
            Activity::Pending("Queued"),
            Activity::Failed("failure".into()),
            Activity::Inactive("Disabled"),
        ] {
            for row in harness.state_mut().iter_mut() {
                row.state = state.clone();
            }
            harness.run();
            for (row, before) in harness.state().iter().zip(&before) {
                assert_eq!(
                    harness.get_by_label(&format!("● {}", row.name)).rect(),
                    *before
                );
            }
        }
    }

    #[test]
    fn freshness_requires_the_exact_document_revision_and_owner() {
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 2, 3);
        assert_eq!(freshness(Some(key), key), Activity::Idle);
        for other in [
            None,
            Some(DocumentKey { revision: 2, ..key }),
            Some(DocumentKey { epoch: 1, ..key }),
            Some(DocumentKey {
                owner: tiptoptyp_core::document::WindowSessionId::new(2),
                ..key
            }),
        ] {
            assert_eq!(freshness(other, key), Activity::Pending("Stale"));
        }
    }
    #[test]
    fn activity_diagnostics_ignore_old_replies_after_typing() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_window_for_tests(
            &context,
            root.path().into(),
            egui::ViewportId::ROOT,
        );
        let uri = "file:///fixture.typ";
        app.tinymist_sync.current_uri = Some(uri.into());
        let old = revision_as_i32(app.document().revision());
        app.receive_editor_diagnostics(uri, Some(old), vec![]);
        assert_eq!(
            freshness(app.activity.diagnostics[0], app.document().key()),
            Activity::Idle
        );
        app.document_mut()
            .edit(CCursorRange::one(CCursor::new(0)), |source| {
                source.push_str("edited")
            });
        app.receive_editor_diagnostics(uri, Some(old), vec![]);
        assert_eq!(
            freshness(app.activity.diagnostics[0], app.document().key()),
            Activity::Pending("Stale")
        );
        let current = revision_as_i32(app.document().revision());
        app.receive_editor_diagnostics("file:///other.typ", Some(current), vec![]);
        assert_eq!(
            freshness(app.activity.diagnostics[0], app.document().key()),
            Activity::Pending("Stale")
        );
        app.receive_editor_diagnostics(uri, Some(current), vec![]);
        assert_eq!(
            freshness(app.activity.diagnostics[0], app.document().key()),
            Activity::Idle
        );
    }

    #[test]
    fn a_new_request_supersedes_failure_but_observing_idle_does_not() {
        let failed = Activity::Failed("disk full".into());
        assert_eq!(queued(failed.clone(), false, "Queued"), failed);
        assert_eq!(queued(failed, true, "Queued"), Activity::Pending("Queued"));
        assert_eq!(queued(Activity::Running, true, "Queued"), Activity::Running);
    }
}
