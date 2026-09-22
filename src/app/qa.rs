//! Privileged QA adapter: the only owner of fixture buffers and scene setup.
//! Normal rendering consumes the same application state and components.
use super::*;
use crate::explorer::ExplorerSection;

#[derive(Default)]
pub(super) struct QaSession {
    document: Option<SceneDocument>,
    font_file: Option<tempfile::NamedTempFile>,
    git_fixture_prepared: bool,
    folding_prepared: bool,
    tabs_prepared: bool,
    asset_fixture: Option<tempfile::TempDir>,
}

/// A capture batch owns its fixture independently of each scene's editor state.
pub(super) struct SceneDocument {
    source: String,
    path: Option<PathBuf>,
    kind: DocumentKind,
    fingerprint: Option<u64>,
}
impl SceneDocument {
    pub(super) fn capture(document: &DocumentSession) -> Self {
        Self {
            source: document.source().clone(),
            path: document.path().clone(),
            kind: document.kind(),
            fingerprint: document.disk_fingerprint(),
        }
    }
    pub(super) fn restore(&self, document: &mut DocumentSession) -> bool {
        if document.config().is_some() {
            document.replace_unprojected_untitled("");
        }
        if document.source() == &self.source
            && document.saved_source() == &self.source
            && document.path() == &self.path
            && document.kind() == self.kind
            && document.disk_fingerprint() == self.fingerprint
        {
            return false;
        }
        if let Some(path) = &self.path {
            document
                .replace_loaded(
                    self.source.clone(),
                    path.clone(),
                    self.kind,
                    self.fingerprint,
                )
                .expect("ordinary QA fixture replacement");
        } else {
            document.replace_unprojected_untitled(self.source.clone());
        }
        true
    }
}

pub(super) const STICKY_CONTEXT_SNAPSHOT_SOURCE: &str = r##"#set page(paper: "a4", margin: 2.2cm)
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

pub(super) const fn source_editor_snapshot_scroll_offset(
    scene: Option<UiSnapshotScene>,
) -> Option<f32> {
    match scene {
        Some(UiSnapshotScene::StickyContext) => Some(STICKY_CONTEXT_SNAPSHOT_SCROLL_OFFSET),
        _ => None,
    }
}

pub(super) fn prepare_sticky_context_snapshot_document(document: &mut DocumentSession) -> bool {
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
    document.set_history_reset(true);
    true
}

impl QaSession {
    pub(super) fn prepare(&mut self, app: &mut EditorApp, context: &egui::Context) {
        let Some(scene) = app.snapshot_scene else {
            return;
        };
        if self.document.is_none() {
            self.document = Some(SceneDocument::capture(app.document()));
        }
        if matches!(
            scene,
            UiSnapshotScene::Main
                | UiSnapshotScene::Tabs
                | UiSnapshotScene::TabsPdf
                | UiSnapshotScene::TabsImage
                | UiSnapshotScene::ProblemsPanel
                | UiSnapshotScene::TerminalPanel
                | UiSnapshotScene::FindReplace
        ) && !app.preview.has_resident_pages()
        {
            app.captures.defer_target("main");
        }
        if let Some(status) =
            settled_snapshot_preview_status(scene, app.preview.has_resident_pages())
        {
            app.preview.status = status;
        }
        if matches!(
            scene,
            UiSnapshotScene::FontCompletion | UiSnapshotScene::SettingsFontPicker
        ) {
            if self.font_file.is_none() {
                let definitions = egui::FontDefinitions::default();
                let key = &definitions.families[&egui::FontFamily::Monospace][0];
                let mut file = tempfile::NamedTempFile::new().expect("QA font fixture");
                std::io::Write::write_all(&mut file, definitions.font_data[key].font.as_ref())
                    .expect("write QA font fixture");
                self.font_file = Some(file);
            }
            app.font_catalog =
                FontCatalog::single_font_fixture(self.font_file.as_ref().unwrap().path());
        }
        let toolbar_anchor = Pos2::new(theme::SPACE.content, METRICS.chrome.toolbar_height);
        match scene {
            UiSnapshotScene::TableEditor | UiSnapshotScene::TableEditorNarrow => {
                if app.table_editor.is_none() {
                    const SOURCE: &str = "= Quarterly review\n\n#table(\n  columns: (2fr, 1fr, 1fr),\n  inset: 8pt,\n  stroke: 0.5pt,\n  table.cell(colspan: 3, fill: rgb(\"#dbeafe\"))[Research programme · 2026],\n  [*Milestone*], [*Owner*], [*Status*],\n  [Literature review], [Ada], [Complete],\n  [Field study], [René], [In progress],\n  [Final report], [Sam], [Planned],\n)\n\nThe code remains selectable and scrollable while the table draft is open.\n";
                    app.document_mut().replace_unprojected_untitled(SOURCE);
                    app.prepare_editor_source_data();
                    let cursor = SOURCE.find("table(").unwrap();
                    app.begin_table_editor(editable_table_at(SOURCE, cursor).unwrap());
                }
            }
            UiSnapshotScene::EmptyWorkspace => {
                if !self.tabs_prepared {
                    app.empty_workspace(context);
                    self.tabs_prepared = true;
                }
            }
            UiSnapshotScene::TabsPdf | UiSnapshotScene::TabsImage => {
                if !self.tabs_prepared {
                    if !app.preview.has_resident_pages() {
                        return;
                    }
                    let (path, kind) = if scene == UiSnapshotScene::TabsPdf {
                        let Some(pdf) = app.preview.content.pdf() else {
                            return;
                        };
                        let directory = tempfile::tempdir().expect("PDF tab fixture directory");
                        let path = directory.path().join("reference.pdf");
                        fs::write(&path, pdf).expect("PDF tab fixture");
                        self.asset_fixture = Some(directory);
                        (path, DocumentKind::Pdf)
                    } else {
                        (
                            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                                .join("assets/icons/tiptoptyp-256.png"),
                            DocumentKind::Image,
                        )
                    };
                    app.prepare_asset_tab_fixture(context, path, kind);
                    self.tabs_prepared = true;
                }
                if !app.asset_preview.has_resident_pages() {
                    app.captures.defer_target("main");
                }
            }
            UiSnapshotScene::Tabs => {
                if !self.tabs_prepared {
                    app.prepare_tabs_fixture(context);
                    self.tabs_prepared = true;
                }
            }
            UiSnapshotScene::GitEditor | UiSnapshotScene::GitChunk => {
                const SOURCE: &str = "= Research notes\n\nThe revised model reaches 96% accuracy.\n\n== Method\nWe evaluate the model on three datasets.\n\nThe additional experiment confirms the result.\n\n== Results\nThe complete comparison follows below.\n\nThe remaining measurements agree.\n\n== Discussion\nThese observations support the revised approach.\n";
                let path = app
                    .document()
                    .path()
                    .clone()
                    .unwrap_or_else(|| app.workspace_root.join("main.typ"));
                if app.document().source() != SOURCE {
                    let disk_fingerprint = app.document().disk_fingerprint();
                    app.document_mut()
                        .replace_loaded(
                            SOURCE.into(),
                            path.clone(),
                            DocumentKind::Typst,
                            disk_fingerprint,
                        )
                        .expect("ordinary QA fixture replacement");
                    app.prepare_editor_source_data();
                }
                app.notice = None;
                app.preview.status = PreviewStatus::Ready(Duration::ZERO);
                app.explorer.open();
                app.view_mode = ViewMode::Code;
                app.git_editor = crate::git::editor::GitEditorState::snapshot_fixture(
                    &app.workspace_root,
                    &path,
                    SOURCE,
                    scene == UiSnapshotScene::GitChunk,
                );
                if scene == UiSnapshotScene::GitChunk
                    && let Some(chunk) = app.git_editor.chunk.clone()
                {
                    app.app_popup = Some(AppPopup::GitChunk {
                        anchor: toolbar_anchor,
                        chunk,
                    });
                }
                app.workspace = Some(WorkspaceTree::from_snapshot(WorkspaceSnapshot {
                    root: app.workspace_root.clone(),
                    nodes: [path, app.workspace_root.join("references.bib")]
                        .into_iter()
                        .map(|path| WorkspaceNode {
                            name: path.file_name().unwrap().to_owned(),
                            relative_path: path
                                .strip_prefix(&app.workspace_root)
                                .unwrap_or(&path)
                                .into(),
                            path,
                            kind: crate::workspace::WorkspaceNodeKind::File,
                            children: Vec::new(),
                        })
                        .collect(),
                }));
            }
            UiSnapshotScene::GitPanel => {
                if !self.git_fixture_prepared {
                    app.git = crate::git::GitPanel::snapshot_fixture();
                    self.git_fixture_prepared = true;
                }
                prepare_git_panel_capture(context, &mut app.explorer);
                app.view_mode = ViewMode::Code;
                app.explorer.set_git_reveal(true);
            }
            UiSnapshotScene::AssetPreview => {}
            UiSnapshotScene::SettingsFontPicker => {
                app.settings_visible = true;
            }
            UiSnapshotScene::UnicodeCompletion => {
                const SOURCE: &str = "= Unicode symbols\n\n$ sym. $\n\nא ב ד ל ∅ ∖ ∀ ⟹ 🜨 ≀\n";
                if app.document().source() != SOURCE {
                    app.document_mut().replace_unprojected_untitled(SOURCE);
                    app.prepare_editor_source_data();
                }
                app.view_mode = ViewMode::Code;
                app.explorer.hide();
                let cursor = SOURCE.find("sym.").unwrap() + 4;
                app.pending_editor_selection = Some(EditorSelection::Focus(cursor..cursor));
                let items = crate::unicode_fonts::SYMBOL_EXAMPLES
                    .into_iter()
                    .map(|(name, symbol)| CompletionItem {
                        label: name.into(),
                        detail: Some(format!("{symbol}, unicode: `\\u{{{:04x}}}`", symbol as u32)),
                        documentation: None,
                        filter_text: None,
                        sort_text: None,
                        insert_text: name.into(),
                        insert_text_is_snippet: false,
                        text_edit: None,
                        additional_text_edits: Vec::new(),
                    })
                    .collect::<Vec<_>>();
                app.editor_completion = Some(EditorCompletionState {
                    key: app.document().key(),
                    provenance: CompletionProvenance::Local,
                    version: revision_as_i32(app.document().revision()),
                    cursor,
                    source_cursor: cursor,
                    anchor: Rect::NOTHING,
                    explicit: true,
                    is_incomplete: false,
                    selected: 4,
                    all_items: items.clone(),
                    items,
                    source: SOURCE.into(),
                });
            }
            UiSnapshotScene::FontCompletion => {
                let source = "#set text(font: \"\")\n= Font completion";
                if app.document().source() != source {
                    app.document_mut().replace_unprojected_untitled(source);
                    app.prepare_editor_source_data();
                }
                app.view_mode = ViewMode::Code;
                app.pending_editor_selection = Some(EditorSelection::Focus(17..17));
                app.request_editor_completion(
                    17,
                    Rect::from_min_size(Pos2::new(300.0, 180.0), Vec2::splat(1.0)),
                    true,
                );
            }
            UiSnapshotScene::Main => {
                app.notice = None;
                app.preview.raw_diagnostics.clear();
                app.preview.diagnostics.clear();
                app.preview.tinymist_diagnostics.clear();
                app.mark_diagnostics_changed();
                app.bottom_panel = BottomPanel::default();
                app.find_bar.visible = false;
                app.find_bar.replace_visible = false;
            }
            UiSnapshotScene::MitexDollars => {
                if app.document().config().is_none() {
                    const SOURCE: &str = "= TeX dollar notation\n\nInline: $\\alpha + \\beta = \\gamma$\n\nDisplay math:\n$\n  \\sum_{n=1}^{\\infty} \\frac{1}{n^2} = \\frac{\\pi^2}{6}\n$\n\nThe saved file contains ordinary MiTeX calls.\n";
                    let canonical = tiptoptyp::mitex_projection::Projection::open(
                        "",
                        tiptoptyp::mitex_projection::Config::default(),
                    )
                    .unwrap()
                    .encode(SOURCE)
                    .unwrap();
                    // A capture fixture must be clean so screenshot-exit is
                    // not intercepted by the unsaved-document close flow.
                    app.document_mut()
                        .replace_unprojected_untitled(canonical.output());
                    app.document_mut()
                        .enable(tiptoptyp::mitex_projection::Config::default())
                        .unwrap();
                    app.prepare_editor_source_data();
                    // The fixture changed file identity to Untitled. Give it
                    // the same private backing/services as a real New document.
                    app.reset_document_services();
                }
                app.view_mode = ViewMode::Code;
                app.explorer.hide();
            }
            UiSnapshotScene::WindowColor => {
                app.view_mode = ViewMode::Code;
            }
            UiSnapshotScene::BracketSettings => {
                app.settings_visible = true;
                app.settings_window.lock().unwrap().ui.scroll_target =
                    Some(SettingsTarget::AutoPairDelimiters);
            }
            UiSnapshotScene::SettingsColors
            | UiSnapshotScene::SettingsEditor
            | UiSnapshotScene::SettingsStatus => {
                app.settings_visible = true;
                app.settings_window.lock().unwrap().ui.scroll_target = Some(match scene {
                    UiSnapshotScene::SettingsColors => SettingsTarget::ThemeColors,
                    UiSnapshotScene::SettingsEditor => SettingsTarget::AutoSaveDelay,
                    _ => SettingsTarget::ToolchainStatus,
                });
            }
            UiSnapshotScene::RainbowBrackets => {
                const SOURCE: &str = "= Rainbow brackets\n\n#let round = (1, (2, (3, (4, (5)))))\n\n#let square = [one #text[two #text[three #text[four #text[five]]]]]\n\n#let curly = { let x = { let y = { 3 }; y }; x }\n\n#let mixed = (1, { [content] }, (2, 3))\n\nMath intervals: $ (0, (1, (2, 3]]] $\n\n// Comments keep their syntax colors: ([{}])\n#let literal = \"[plain string]\"\nRaw text: `([{}])`\n";
                if app.document().source() != SOURCE {
                    app.document_mut().replace_unprojected_untitled(SOURCE);
                    app.prepare_editor_source_data();
                }
                app.settings.rainbow_brackets = crate::rainbow::RainbowBrackets::default();
                app.explorer.hide();
                app.view_mode = ViewMode::Code;
            }
            UiSnapshotScene::DelimiterMatch => {
                const SOURCE: &str = "= Matching delimiters\n\n#let calculate(value) = {\n  let nested = (value, (2, 3))\n  nested\n}\n\nStrings are separate: #repr(\"[literal]\")\nMath: $ (alpha + beta] $\n";
                if app.document().source() != SOURCE {
                    app.document_mut().replace_unprojected_untitled(SOURCE);
                    app.prepare_editor_source_data();
                }
                app.view_mode = ViewMode::Code;
                let cursor = SOURCE.find('{').unwrap();
                app.pending_editor_selection = Some(EditorSelection::Focus(cursor..cursor));
            }
            UiSnapshotScene::StickyContext => {
                app.notice = None;
                app.view_mode = ViewMode::Code;
                app.bottom_panel = BottomPanel::default();
                app.find_bar.visible = false;
                app.find_bar.replace_visible = false;
                if prepare_sticky_context_snapshot_document(app.document_mut()) {
                    app.prepare_editor_source_data();
                }
            }
            UiSnapshotScene::Folding => {
                if !self.folding_prepared {
                    const SOURCE: &str = "#let cmarker = (render: x => x)\n#let mitext(x) = x\n\n= Folding · Unicode αβ\n\n#let compact(x) = {\n  let y = x + 1\n  y * y\n}\n\n#let expanded(x) = {\n  x + 1\n}\n\n#(cmarker.render)(`\n# Markdown section\nThis body is folded.\n## Nested heading\nNested content.\n# Visible sibling\nVisible Markdown text.\n`)\n\n#mitext(`\n\\section{TeX section}\n\\begin{align}\nx &= y + z \\\\\n\\end{align}\n\\section{Visible sibling}\nVisible TeX text.\n`)\n\n= Final section\nThe original source positions are preserved.\n";
                    app.document_mut()
                        .edit(CCursorRange::one(CCursor::new(0)), |source| {
                            *source = SOURCE.to_owned()
                        });
                    app.document_mut().set_history_reset(true);
                    app.prepare_editor_source_data();
                    let regions = app.editor_data.context_regions();
                    let key = app.document().key();
                    let source = app.editor_data.source_snapshot();
                    app.folding_mut().prepare(key, source, &regions);
                    for needle in [
                        "#let compact",
                        "# Markdown section",
                        "\\section{TeX section}",
                    ] {
                        let line = SOURCE[..SOURCE.find(needle).unwrap()]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count();
                        app.folding_mut().toggle(line);
                    }
                    if let Some(path) = app.document().path() {
                        app.git_editor = crate::git::editor::GitEditorState::snapshot_fixture(
                            &app.workspace_root,
                            path,
                            SOURCE,
                            false,
                        );
                    }
                    self.folding_prepared = true;
                }
                app.view_mode = ViewMode::Code;
                app.bottom_panel = BottomPanel::default();
                app.notice = None;
            }
            UiSnapshotScene::FileMenu => {
                app.app_popup = Some(AppPopup::File {
                    anchor: toolbar_anchor + egui::vec2(150.0, 0.0),
                });
            }
            UiSnapshotScene::EditMenu => {
                app.app_popup = Some(AppPopup::Edit {
                    anchor: toolbar_anchor + egui::vec2(195.0, 0.0),
                });
            }
            UiSnapshotScene::SettingsWindow
            | UiSnapshotScene::SettingsThemePicker
            | UiSnapshotScene::SettingsDarkThemePicker
            | UiSnapshotScene::SettingsTooltip => app.settings_visible = true,
            UiSnapshotScene::TypstOverridesWindow => {
                app.settings_visible = false;
                app.typst_overrides_visible = true;
                app.typst_overrides_dark = app
                    .imported_theme
                    .as_ref()
                    .is_none_or(|theme| theme.dark_mode);
            }
            // These scenes are injected after the editor paints, because the
            // editor intentionally replaces hover overlays every frame.
            UiSnapshotScene::DiagnosticTooltip | UiSnapshotScene::FunctionTooltip => {}
            UiSnapshotScene::SaveDialog => {
                if app.document_workflow.modal().is_none() {
                    app.document_workflow.set_modal(AppModal::Unsaved {
                        message: format!(
                            "Save changes to {} before opening another file?",
                            app.document_name()
                        ),
                        pending: PendingDocumentAction {
                            action: DeferredDocumentAction::CloseWindow,
                            key: app.document().key(),
                            allow_discard: true,
                            description: "closing the document".to_owned(),
                        },
                    });
                }
            }
            UiSnapshotScene::AlertDialog => {
                if app.document_workflow.modal().is_none() {
                    app.document_workflow.set_modal(AppModal::Alert {
                        title: "error".to_owned(),
                        message:
                            "The document could not be saved. Check the destination and try again."
                                .to_owned(),
                        kind: NoticeKind::Error,
                    });
                }
            }
            UiSnapshotScene::OverwriteDialog => {
                if app.document_workflow.modal().is_none() {
                    app.document_workflow.set_modal(AppModal::Overwrite {
                        message: "This file changed on disk after it was opened. Overwrite it with the editor contents?"
                            .to_owned(),
                        path: app.document().path()
                            .clone()
                            .unwrap_or_else(|| app.workspace_root.join("document.typ")),
                        key: app.document().key(),
                        expected_disk_fingerprint: Some(1),
                        observed_disk_fingerprint: Some(2),
                    });
                }
            }
            UiSnapshotScene::EditorContextMenu => {
                app.app_popup = Some(AppPopup::Editor {
                    anchor: Pos2::new(430.0, 250.0),
                    link: None,
                    table: None,
                });
            }
            UiSnapshotScene::DocumentFontSelector => {
                app.document_mut()
                    .edit(CCursorRange::one(CCursor::new(0)), |source| {
                        *source =
                            "#set text(font: \"Libertinus Serif\")\n= Font selector".to_owned()
                    });
                app.prepare_editor_source_data();
                let font_char = app.document().source()
                    [..app.document().source().find("font").unwrap()]
                    .chars()
                    .count();
                let target = app
                    .editor_data
                    .font_argument_at(font_char)
                    .expect("snapshot font argument is valid");
                app.font_catalog = FontCatalog::snapshot_fixture();
                app.app_popup = Some(AppPopup::FontSelector {
                    anchor: Pos2::new(430.0, 250.0),
                    target,
                });
            }
            UiSnapshotScene::ExplorerContextMenu => {
                let path = app
                    .document()
                    .path()
                    .clone()
                    .unwrap_or_else(|| app.workspace_root.join("document.typ"));
                app.app_popup = Some(AppPopup::Workspace {
                    anchor: Pos2::new(180.0, 180.0),
                    path,
                    is_file: true,
                });
            }
            UiSnapshotScene::StatusLog => {
                app.preview.status = PreviewStatus::Ready(Duration::from_millis(18));
                app.recorded_status = Some(app.preview.status);
                app.notice = None;
                app.recorded_notice = None;
                app.status_log = VecDeque::from([
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
                app.app_popup = Some(AppPopup::StatusLog {
                    anchor: Pos2::new(theme::SPACE.content, 500.0),
                });
            }
            UiSnapshotScene::RenameDialog => {
                if app.rename_dialog.is_none() {
                    let path = app
                        .document()
                        .path()
                        .clone()
                        .unwrap_or_else(|| app.workspace_root.join("document.typ"));
                    let name = path.file_name().map_or_else(
                        || "document.typ".to_owned(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                    app.rename_dialog = Some(RenameDialog {
                        path,
                        name,
                        focus: false,
                    });
                }
            }
            UiSnapshotScene::WorkspaceChooser => app.workspace_chooser_visible = true,
            UiSnapshotScene::ExplorerMaximized => {
                app.explorer.open();
                app.view_mode = ViewMode::Code;
                if app.explorer.maximized_section(true) != Some(ExplorerSection::Files) {
                    app.explorer
                        .toggle_section_maximized(ExplorerSection::Files);
                }
            }
            UiSnapshotScene::TerminalPanel | UiSnapshotScene::TerminalMaximized => {
                if scene == UiSnapshotScene::TerminalMaximized && !app.bottom_panel.is_maximized() {
                    app.bottom_panel.toggle_maximized();
                }
                app.bottom_panel.select(PanelTab::Terminal);
                app.terminal.prepare_fixture(
                    "\x1b[?2027h\x1b[32m~/project\x1b[0m $ typst compile notes.typ\r\n\x1b[32mCompilation finished\x1b[0m in 42 ms\r\n\x1b[1;35mgit_branch master\x1b[0m  \x1b[1;33mpackage v0.1.0\x1b[0m  \x1b[1;31mrust v1.98.1\x1b[0m\r\nDigits: 0123456789 #*  \x1b[1mbold 0123456789\x1b[0m  \x1b[3mitalic 0123456789\x1b[0m\r\n\x1b[1mGhostty terminal\x1b[0m  \x1b[31mred\x1b[0m  \x1b[34mblue\x1b[0m  \x1b[38;2;180;90;200mtrue color\x1b[0m\r\nUnicode: α + β = γ   é   界   😀 🦀 📦 👩‍💻 🇬🇧\r\n\x1b[32m~/project\x1b[0m $ ".as_bytes(),
                    Path::new("~/project"),
                );
            }
            UiSnapshotScene::ProblemsPanel => {
                app.bottom_panel.select(PanelTab::Problems);
                app.preview.diagnostics = vec![
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
                app.preview.tinymist_diagnostics.clear();
                app.preview.raw_diagnostics.clear();
                app.mark_diagnostics_changed();
            }
            UiSnapshotScene::FindReplace => {
                app.notice = None;
                app.find_bar.visible = true;
                app.find_bar.replace_visible = true;
                app.find_bar.query = "Typst".to_owned();
                app.find_bar.replacement = "tiptoptyp".to_owned();
            }
            UiSnapshotScene::PreviewCompiling => {
                app.preview.status = PreviewStatus::Compiling;
            }
        }
    }

    /// Move a deterministic capture session to its next isolated UI state.
    /// Transient windows from the previous scene are closed before the new
    /// scene is painted, while the loaded fixture and rendered preview remain
    /// available across the whole process.
    pub(super) fn set_step(
        &mut self,
        app: &mut EditorApp,
        step: &UiCaptureStep,
        context: &egui::Context,
    ) {
        crate::window_logo::clear_snapshot(context);
        self.git_fixture_prepared = false;
        app.git.visible = false;
        app.git_editor = crate::git::editor::GitEditorState::default();
        app.settings_visible = false;
        app.shortcut_editor_visible = false;
        app.packages_visible = false;
        app.typst_overrides_visible = false;
        app.workspace_chooser_visible = false;
        app.bottom_panel = BottomPanel::default();
        if let Some(section) = app.explorer.maximized_section(true) {
            app.explorer.toggle_section_maximized(section);
        }
        app.terminal = TerminalPane::default();
        app.find_bar.visible = false;
        app.find_bar.replace_visible = false;
        app.view_mode = ViewMode::Split;
        app.find_bar.search.clear();
        app.find_bar.focus = false;
        app.pending_editor_selection = None;
        app.diagnostic_tooltip = None;
        app.close_app_popup();
        app.document_workflow.clear_modal();
        app.rename_dialog = None;
        app.table_editor = None;
        app.rename_overlay_had_focus = false;
        app.rename_overlay_suspended = false;
        app.settings_window.lock().unwrap().ui.staged_ui_font_weight = None;
        app.settings_window
            .lock()
            .unwrap()
            .ui
            .staged_code_font_weight = None;
        app.notice = None;
        app.preview.raw_diagnostics.clear();
        app.preview.diagnostics.clear();
        app.preview.tinymist_diagnostics.clear();
        app.mark_diagnostics_changed();
        app.status_log.clear();
        app.recorded_status = None;
        app.recorded_notice = None;
        app.preview.status = PreviewStatus::Ready(Duration::ZERO);
        if let Some(fixture) = &self.document {
            // Keep a real, owned document tab. Restoring into Tabs::default's
            // empty-workspace placeholder disables preview processing and
            // leaves every subsequent main capture waiting forever.
            let owner = app.document().key().owner;
            let document = std::mem::replace(
                app.document_mut(),
                DocumentSession::new(owner, "", DocumentKind::Typst),
            );
            app.tabs = tabs::Tabs::new(false, document, app.workspace_root.clone());
            if fixture.restore(app.document_mut()) {
                app.preview.content.clear();
            }
        } else {
            app.document_mut()
                .restore_saved_source()
                .expect("ordinary QA fixture revert");
        }
        app.theme_override = Some(step.theme.clone());
        app.snapshot_scene = Some(step.scene);
        self.folding_prepared = false;
        self.tabs_prepared = false;
        self.asset_fixture = None;
        if step.scene == UiSnapshotScene::StickyContext {
            app.document_mut().set_history_reset(true);
        }

        if matches!(
            step.scene,
            UiSnapshotScene::Main
                | UiSnapshotScene::Tabs
                | UiSnapshotScene::TabsPdf
                | UiSnapshotScene::TabsImage
                | UiSnapshotScene::ProblemsPanel
                | UiSnapshotScene::TerminalPanel
                | UiSnapshotScene::FindReplace
        ) && !app.preview.has_resident_pages()
        {
            app.schedule_compile_now();
        }
        context.request_repaint();
    }
}

/// Child-window captures can leave a collapsed root-panel size in egui memory.
/// Seed the real resizable Explorer with a deterministic, visible fixture width.
fn prepare_git_panel_capture(context: &egui::Context, explorer: &mut ExplorerPanelState) {
    explorer.open();
    let id = explorer_panel_id(context);
    let rect = egui::PanelState::load(context, id)
        .map_or(context.content_rect(), |state| state.outer_rect);
    let outer_rect = explorer_width_restored_rect(rect, METRICS.chrome.explorer_default_width);
    context.data_mut(|data| data.insert_persisted(id, egui::PanelState { outer_rect }));
    explorer.remember_width(METRICS.chrome.explorer_default_width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn serial_capture_restores_a_live_document_tab_and_preserves_unchanged_identity() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let mut qa = QaSession {
            document: Some(SceneDocument::capture(app.document())),
            ..Default::default()
        };
        let key = app.document().key();
        for theme in ["tiptop-light", "tiptop-dark"] {
            qa.set_step(
                &mut app,
                &UiCaptureStep {
                    theme: CaptureThemeProfile {
                        name: theme.into(),
                        invert: false,
                        hue_shift_degrees: 0,
                    },
                    scene: UiSnapshotScene::Main,
                },
                &context,
            );
            assert_eq!(app.tabs.len(), 1);
            assert!(app.typst_preview_available());
            assert_eq!(app.document().key(), key);
        }
        app.document_mut()
            .replace_unprojected_untitled("scene-only buffer");
        qa.set_step(
            &mut app,
            &UiCaptureStep {
                theme: CaptureThemeProfile {
                    name: "tiptop-light".into(),
                    invert: false,
                    hue_shift_degrees: 0,
                },
                scene: UiSnapshotScene::Main,
            },
            &context,
        );
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.document().source(), &qa.document.unwrap().source);
        assert_eq!(app.document().key().owner, key.owner);
        assert!(app.typst_preview_available());
    }

    #[test]
    fn git_capture_restores_collapsed_explorer_and_renders_real_repository_controls() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(900.0, 700.0))
            .build_ui_state(
                |ui, state: &mut (ExplorerPanelState, crate::git::GitPanel)| {
                    // Simulate root geometry left by a preceding child-window scene.
                    let panel_id = explorer_panel_id(ui.ctx());
                    ui.ctx().data_mut(|data| {
                        data.insert_persisted(
                            panel_id,
                            egui::PanelState {
                                outer_rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(12.0, 700.0)),
                            },
                        )
                    });
                    prepare_git_panel_capture(ui.ctx(), &mut state.0);
                    egui::Panel::left(panel_id)
                        .frame(theme::content_panel_frame(ui.style()))
                        .default_size(METRICS.chrome.explorer_default_width)
                        .min_size(METRICS.chrome.explorer_min_width)
                        .show(ui, |ui| {
                            let mut content = clipped_panel_content_ui(ui, "git-capture-test");
                            let _ = state.1.show(
                                &mut content,
                                false,
                                crate::settings::GitDiffStyle::Unified,
                            );
                        });
                },
                (
                    ExplorerPanelState::default(),
                    crate::git::GitPanel::snapshot_fixture(),
                ),
            );
        harness.run();
        let width = egui::PanelState::load(&harness.ctx, explorer_panel_id(&harness.ctx))
            .unwrap()
            .size()
            .x;
        assert!(
            (width - METRICS.chrome.explorer_default_width).abs() < 1.0,
            "{width}"
        );
        assert!(harness.get_by_label("Fetch").rect().right() <= width);
        assert!(harness.get_by_label("Stage all").rect().left() >= 0.0);
        assert!(
            harness.get_by_label("4 files · 2 staged").rect().bottom()
                <= harness.get_by_label("Stage all").rect().top()
        );
    }
}
