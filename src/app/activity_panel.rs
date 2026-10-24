//! Activity is observed, never driven, by this panel. Opening it starts no jobs.
use super::*;
use crate::activity::Activity;
mod filter;
#[cfg(test)]
mod tests;
mod view;
use super::filter_bar::{ScrollLayout, initial_offset};
use filter::{Filter, Group, Status};
use std::collections::HashSet;
use view::show_indicators;

#[derive(Default)]
pub(super) struct ActivityTracking {
    pub(super) view: State,
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
#[derive(Clone, Debug, PartialEq, Eq)]
struct Indicator {
    group: Group,
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
        let mut rows = Vec::with_capacity(32);
        let mut add = |group, name, role, state| {
            rows.push(Indicator {
                group,
                name,
                role,
                state,
            })
        };
        add(
            Group::Code,
            "Handwriting",
            "Recognizes symbols drawn in Explorer using the offline model.",
            self.symbol_drawing.activity(),
        );
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
            Group::Files,
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
            Group::Preview,
            "Typst",
            "Builds a PDF from your Typst document.",
            if self.preview_document_kind() == DocumentKind::Typst {
                build.clone()
            } else {
                inactive.clone()
            },
        );
        add(
            Group::Preview,
            self.settings.tex.build_engine.label(),
            "Builds a PDF from your TeX document.",
            if self.preview_document_kind() == DocumentKind::Tex && self.settings.tex.build_enabled
            {
                build
            } else {
                inactive.clone()
            },
        );
        add(
            Group::Preview,
            "SyncTeX",
            "Connects TeX source positions to matching places in the PDF and back.",
            if self.preview_document_kind() != DocumentKind::Tex {
                inactive.clone()
            } else if self.tex_tools.synctex.is_none() {
                Activity::Inactive("Install the synctex command")
            } else if self.synctex.artifact.is_none() {
                Activity::Inactive("Compile a source map first")
            } else {
                self.synctex.activity()
            },
        );
        let intelligence_busy = self.activity.completion.is_some() || self.activity.hover.is_some();
        let tinymist = if typst {
            service(&self.preview.tinymist_state)
        } else {
            inactive.clone()
        };
        add(
            Group::Code,
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
            Group::Preview,
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
            Group::Checks,
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
                Group::Code,
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
            Group::Code,
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
            Group::Checks,
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
            Group::Preview,
            "Vector view",
            "Shows Tinymist's live preview inside the preview pane.",
            service(&self.preview.webview_state),
        );
        add(
            Group::Preview,
            "PDFium preview",
            "Draws compiled PDF pages when the PDFium preview is selected.",
            self.pdfium_preview.activity(),
        );
        add(
            Group::Preview,
            "Opened PDF",
            "Draws and searches a PDF file opened as a document.",
            self.pdfium_asset.activity(),
        );
        add(
            Group::Files,
            "File loading",
            "Loads an opened PDF or image before it can be shown.",
            if self.document().kind().preview_only() {
                preview_work(self.asset_preview.status)
            } else {
                inactive.clone()
            },
        );
        add(
            Group::Files,
            "Hover thumbnail",
            "Makes the small image or PDF preview shown on hover.",
            self.hover_activity(),
        );
        add(
            Group::Git,
            "Git status",
            "Checks changed files and handles Git repository actions.",
            self.git.activity(),
        );
        add(
            Group::Git,
            "Git hunks",
            "Marks changed lines beside your source code.",
            self.git_editor.activity(),
        );
        add(
            Group::Git,
            "Git mutation",
            "Stages, unstages, or discards selected changes.",
            self.git_hunk_job.activity(),
        );
        add(
            Group::Files,
            "Workspace",
            "Refreshes the workspace file list when files change.",
            if self.workspace_service.is_subscribed() {
                self.activity.workspace.clone()
            } else {
                inactive.clone()
            },
        );
        add(
            Group::Files,
            "File watcher",
            "Notices files changed outside the app.",
            if self.workspace_service.is_subscribed() {
                self.activity.watcher.clone()
            } else {
                inactive.clone()
            },
        );
        add(
            Group::Files,
            "Project index",
            "Finds headings, labels, and references across your project.",
            Activity::work(
                self.project_index_job.is_running(),
                self.project_index_deadline.is_pending(),
                self.activity.index_error.as_deref(),
            ),
        );
        add(
            Group::Checks,
            "LTeX+",
            "Checks spelling and grammar with the optional local LanguageTool server.",
            self.ltex.activity(),
        );
        add(
            Group::Checks,
            "Harper",
            "English spelling and grammar checks.",
            if self.settings.english_grammar {
                self.writing.activity()
            } else {
                inactive.clone()
            },
        );
        add(
            Group::Checks,
            "Unicode",
            "Flags invisible or easily confused characters.",
            if self.settings.unicode_warnings {
                self.writing.activity()
            } else {
                inactive.clone()
            },
        );
        add(
            Group::Tools,
            "Fonts",
            "Finds fonts available in your workspace.",
            self.font_catalog_scan.activity(),
        );
        add(
            Group::Tools,
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
            Group::Tools,
            "Package removal",
            "Removes installed packages.",
            self.package_uninstall.activity(),
        );
        add(
            Group::Files,
            "File operations",
            "Creates, duplicates and imports workspace files.",
            queued(
                self.file_import.activity(),
                !self.queued_file_drops.is_empty(),
                "Queued",
            ),
        );
        add(
            Group::Tools,
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
        show_indicators(ui, &mut self.activity.view, &rows);
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
fn fixture_indicators() -> Vec<Indicator> {
    [
        ("Save", Activity::Pending("Save queued")),
        ("Typst", Activity::Running),
        ("Tectonic", Activity::Inactive("Not in use")),
        ("Handwriting", Activity::Inactive("Not in use")),
        ("SyncTeX", Activity::Inactive("Not in use")),
        ("Tinymist", Activity::Idle),
        ("Live preview", Activity::Running),
        ("Typst diagnostics", Activity::Pending("Stale")),
        ("TexLab", Activity::Inactive("Not in use")),
        ("Badness", Activity::Inactive("Not in use")),
        ("Tinymist format", Activity::Idle),
        ("Compiler diagnostics", Activity::Pending("Stale")),
        ("Vector view", Activity::Idle),
        ("PDFium preview", Activity::Inactive("Not in use")),
        ("Opened PDF", Activity::Inactive("Not in use")),
        ("File loading", Activity::Idle),
        ("Hover thumbnail", Activity::Inactive("Not in use")),
        (
            "Git status",
            Activity::Failed("Example: Git executable unavailable".into()),
        ),
        ("Git hunks", Activity::Pending("Stale")),
        ("Git mutation", Activity::Idle),
        ("Workspace", Activity::Idle),
        ("File watcher", Activity::Idle),
        ("Project index", Activity::Running),
        ("LTeX+", Activity::Idle),
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
        group: fixture_group(name),
        name,
        role: "Deterministic QA example",
        state,
    })
    .collect()
}

fn fixture_group(name: &str) -> Group {
    match name {
        "Typst" | "Tectonic" | "SyncTeX" | "Live preview" | "Vector view" | "PDFium preview"
        | "Opened PDF" => Group::Preview,
        "Handwriting" | "Tinymist" | "TexLab" | "Badness" | "Tinymist format" => Group::Code,
        "Typst diagnostics" | "Compiler diagnostics" | "LTeX+" | "Harper" | "Unicode" => {
            Group::Checks
        }
        "Git status" | "Git hunks" | "Git mutation" => Group::Git,
        "Fonts" | "Packages" | "Package removal" | "Terminal" => Group::Tools,
        _ => Group::Files,
    }
}

pub(super) struct State {
    filter: Filter,
    grouped: bool,
    collapsed: HashSet<Group>,
    pub(super) focus_search: bool,
    pub(super) popup_open: bool,
    pub(super) controls_visible: bool,
    pub(super) sticky_filters: bool,
    pub(super) scroll_offset: f32,
    pub(super) scroll_rect: Rect,
    pub(super) controls_rect: Rect,
    controls_height: f32,
    scroll_layout: Option<ScrollLayout>,
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    inventory: Vec<Indicator>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            filter: Filter::default(),
            grouped: true,
            collapsed: HashSet::new(),
            focus_search: false,
            popup_open: false,
            controls_visible: false,
            sticky_filters: false,
            scroll_offset: 0.0,
            scroll_rect: Rect::NOTHING,
            controls_rect: Rect::NOTHING,
            controls_height: 0.0,
            scroll_layout: None,
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            inventory: Vec::new(),
        }
    }
}
pub(super) fn search_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "activity-search")
}
#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
impl State {
    pub(super) fn inspect(&self) -> serde_json::Value {
        let indices = self.filter.filter_indices(&self.inventory);
        let groups = Group::ALL.iter().filter_map(|group| {
            let count = indices.iter().filter(|&&index| self.inventory[index].group == *group).count();
            if count == 0 {return None;}
            Some(serde_json::json!({"id":format!("{group:?}"),"label":group.label(),"count":count,
                "open":!self.collapsed.contains(group),"target":format!("activity.group.{group:?}")}))
        }).collect::<Vec<_>>();
        let displayed_count = indices
            .iter()
            .filter(|&&index| {
                !self.grouped || !self.collapsed.contains(&self.inventory[index].group)
            })
            .count();
        serde_json::json!({"query":self.filter.query,"group":self.filter.group.map(|g|format!("{g:?}")),
            "status":self.filter.status.map(|s|format!("{s:?}")),"grouped":self.grouped,
            "visible_count":indices.len(),"total_count":self.inventory.len(),"displayed_count":displayed_count,
            "visible_names":indices.iter().map(|&i|self.inventory[i].name).collect::<Vec<_>>(),"groups":groups,
            "rows":self.inventory.iter().map(|row| serde_json::json!({"name":row.name,
                "group":format!("{:?}",row.group),"state":format!("{:?}",Status::of(&row.state)),
                "detail":match &row.state {Activity::Failed(error)=>format!("Failed {error}"), _=>row.state.label().into()}})).collect::<Vec<_>>(),
            "controls_visible":self.controls_visible,"sticky_filters":self.sticky_filters})
    }
}
