//! One draft lifetime and Apply/Cancel pipeline for Typst and TeX tables.
use super::*;
use crate::editor_features::{TableSource, TexTable};

#[derive(Debug, Clone)]
pub(super) enum TableEditorDialog {
    Typst(Box<TypstTableEditorDialog>),
    Source(Box<SourceDialog>),
    Tex(Box<TexDialog>),
}
#[derive(Debug, Clone)]
pub(super) struct SourceDialog {
    pub(super) target: TableSource,
    draft: String,
    key: DocumentKey,
    error: Option<String>,
    validation: Option<String>,
}
#[derive(Debug, Clone)]
pub(super) struct TexDialog {
    pub(super) table: TexTable,
    original: String,
    draft: String,
    column_spec: String,
    pub(super) source_mode: bool,
    key: DocumentKey,
    error: Option<String>,
    validation: Option<String>,
}
impl TableEditorDialog {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn desktop_inspection(&self) -> serde_json::Value {
        let (draft, valid) = match self {
            Self::Source(dialog) => (&dialog.draft, dialog.validation.is_none()),
            Self::Tex(dialog) if dialog.source_mode => (&dialog.draft, dialog.validation.is_none()),
            _ => return serde_json::Value::Null,
        };
        serde_json::json!({
            "fingerprint": crate::desktop_test::fingerprint(draft),
            "bytes": draft.len(),
            "valid": valid,
        })
    }
    pub(super) fn set_error(&mut self, error: String) {
        match self {
            Self::Typst(dialog) => dialog.error = Some(error),
            Self::Source(dialog) => dialog.error = Some(error),
            Self::Tex(dialog) => dialog.error = Some(error),
        }
    }
}
impl EditorApp {
    pub(super) fn cancel_table_editor(&mut self, context: &egui::Context) {
        self.table_editor = None;
        self.table_samples.reset();
        crate::table_sampling::clear(
            context,
            scoped_child_viewport_id(context, "tiptoptyp-table-editor"),
        );
        self.table_edit_events.clear();
        ChildViewHost::close(context, "tiptoptyp-table-editor");
        crate::window_host::return_focus(context);
    }
    pub(super) fn begin_table_target(&mut self, mut target: TableTarget) {
        if self.table_editor.is_some() {
            return;
        }
        self.table_samples.reset();
        if let TableTarget::Typst(table) = &target
            && !table.has_explicit_columns()
            && table
                .cells
                .iter()
                .flatten()
                .any(|cell| crate::table_sampling::computed_markup(cell))
        {
            target = TableTarget::Source(TableSource {
                kind: table.kind,
                source_range: table.source_range.clone(),
                source: char_range_slice(self.document().source(), table.source_range.clone())
                    .unwrap_or_default()
                    .into(),
            });
        }
        self.table_edit_events.clear();
        let key = self.document().key();
        self.table_editor = match target {
            TableTarget::Typst(table) => {
                self.begin_table_editor(table);
                return;
            }
            TableTarget::Source(target) => {
                Some(TableEditorDialog::Source(Box::new(SourceDialog {
                    draft: target.source.clone(),
                    target,
                    key,
                    error: None,
                    validation: None,
                })))
            }
            TableTarget::Tex(table) => Some(TableEditorDialog::Tex(Box::new(TexDialog {
                draft: table.source.clone(),
                original: table.source.clone(),
                column_spec: table.column_spec.clone(),
                source_mode: table.cells.is_none(),
                validation: table.grid_source_edit().err(),
                table,
                key,
                error: None,
            }))),
        };
        self.editor_completion = None;
        self.format_request_key = None;
        self.manual_format_revision = None;
    }
}

pub(super) fn prepare_child_keyboard(
    ui: &mut egui::Ui,
    dialog: &mut TableEditorDialog,
    shortcuts: &ShortcutBindings,
    allow_requested_paste: bool,
) -> ChildWidgetEdits {
    if let TableEditorDialog::Typst(dialog) = dialog {
        table_editor::prepare_grid_keyboard(ui, dialog);
    }
    prepare_child_widget_edit_events(ui.ctx(), shortcuts, allow_requested_paste)
}

pub(super) fn prepare_source_edit(
    source: &str,
    key: DocumentKey,
    dialog: &TableEditorDialog,
) -> Result<PreparedTableSourceEdit, String> {
    let (expected_key, original, edit) = match dialog {
        TableEditorDialog::Typst(dialog) => return prepare_table_source_edit(source, key, dialog),
        TableEditorDialog::Source(dialog) => (
            dialog.key,
            dialog.target.source.as_str(),
            dialog.target.source_edit(&dialog.draft),
        ),
        TableEditorDialog::Tex(dialog) => (
            dialog.key,
            dialog.original.as_str(),
            if dialog.source_mode {
                dialog.table.source_edit(&dialog.draft)
            } else {
                dialog.table.grid_source_edit()
            },
        ),
    };
    if key != expected_key {
        return Err("The document changed while the table editor was open. Reopen the table to edit the latest source.".into());
    }
    let edit = edit?;
    let byte_range = char_range_to_byte_range(source, edit.range.clone())
        .ok_or("The table range is no longer valid")?;
    if source.get(byte_range.clone()) != Some(original) {
        return Err(
            "The table source changed while the table editor was open. Reopen it to continue."
                .into(),
        );
    }
    let cursor = edit.range.start + edit.replacement.chars().count();
    if matches!(dialog, TableEditorDialog::Tex(_)) {
        let table = crate::editor_features::tex_table_at(&edit.replacement, 1)
            .ok_or("Finish the table source before applying it")?;
        let required = crate::tex::imports::Requirements::table(&table, &edit.replacement);
        let edit = crate::tex::imports::prepare_edit(
            source,
            byte_range,
            &edit.replacement,
            cursor,
            &required,
        )?;
        return Ok(PreparedTableSourceEdit {
            cursor: edit.cursor,
            byte_range: edit.byte_range,
            replacement: edit.replacement,
        });
    }
    Ok(PreparedTableSourceEdit {
        cursor,
        byte_range,
        replacement: edit.replacement,
    })
}

pub(super) fn show(
    ui: &mut egui::Ui,
    dialog: &mut TableEditorDialog,
    width: f32,
    height: f32,
    settings: &AppSettings,
) -> Option<TableEditorUiAction> {
    if let TableEditorDialog::Typst(typst) = dialog
        && typst.insertion_prefix.is_none()
        && icons::action_button(ui, "Source")
            .on_hover_text(
                "Edit the complete table expression, including computed options and partial rules",
            )
            .clicked()
    {
        match typst.table.render_source_draft() {
            Ok(draft) => {
                let target = TableSource {
                    kind: typst.table.kind,
                    source_range: typst.table.source_range.clone(),
                    source: typst.original_call.clone(),
                };
                crate::table_sampling::set_source_mode(ui.ctx(), true);
                *dialog = TableEditorDialog::Source(Box::new(SourceDialog {
                    validation: target.source_edit(&draft).err(),
                    target,
                    draft,
                    key: typst.document_key,
                    error: None,
                }));
            }
            Err(error) => typst.error = Some(error.to_string()),
        }
    }
    match dialog {
        TableEditorDialog::Typst(dialog) => {
            show_table_editor_ui(ui, dialog, width, height, settings)
        }
        TableEditorDialog::Source(dialog) => {
            let view = crate::table_sampling::view(ui);
            let mode_id = crate::table_sampling::mode_key(ui.ctx().viewport_id());
            let mut source_mode = ui
                .ctx()
                .data(|data| data.get_temp::<bool>(mode_id))
                .unwrap_or(view.is_none());
            let toggle = ui
                .add_enabled_ui(view.is_some(), |ui| {
                    ui.toggle_value(&mut source_mode, "Source")
                })
                .inner;
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("table.source-mode", &toggle);
            let _ = toggle;
            ui.ctx()
                .data_mut(|data| data.insert_temp(mode_id, source_mode));
            if !source_mode {
                table_samples::sampled_grid(ui, height);
                return footer(ui, &dialog.error, dialog.validation.clone());
            }
            ui.strong(format!("{} · source", dialog.target.kind.callee()));
            ui.label("Computed cells and options stay intact. Edit the complete call below.");
            if source_field(
                ui,
                "table-source",
                &mut dialog.draft,
                "typ-code",
                height - 130.0,
                settings,
            ) {
                dialog.validation = dialog.target.source_edit(&dialog.draft).err();
                dialog.error = None;
            }
            footer(ui, &dialog.error, dialog.validation.clone())
        }
        TableEditorDialog::Tex(dialog) => show_tex(ui, dialog, height, settings),
    }
}
fn source_field(
    ui: &mut egui::Ui,
    salt: &str,
    draft: &mut String,
    extension: &str,
    height: f32,
    settings: &AppSettings,
) -> bool {
    egui::ScrollArea::both()
        .id_salt(salt)
        .max_height(height.max(70.0))
        .show(ui, |ui| {
            let _response = code_editor::editor(ui, salt, draft, extension, settings, 12);
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("table.source", &_response);
            _response.changed()
        })
        .inner
}
fn footer(
    ui: &mut egui::Ui,
    error: &Option<String>,
    validation: Option<String>,
) -> Option<TableEditorUiAction> {
    if let Some(error) = error.as_ref().or(validation.as_ref()) {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.horizontal(|ui| {
        let mut action = None;
        let apply = icons::action_button_enabled(ui, validation.is_none(), "Apply");
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("table.apply", &apply);
        if apply.clicked() {
            action = Some(TableEditorUiAction::Apply);
        }
        let cancel = icons::action_button(ui, "Cancel");
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("table.cancel", &cancel);
        if cancel.clicked() {
            action = Some(TableEditorUiAction::Cancel);
        }
        action
    })
    .inner
}
fn show_tex(
    ui: &mut egui::Ui,
    dialog: &mut TexDialog,
    height: f32,
    settings: &AppSettings,
) -> Option<TableEditorUiAction> {
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("TeX · {}", dialog.table.environment.name())).on_hover_text("Missing packages and tabularray libraries are added to the preamble on Apply, in the same undoable edit. Existing imports and options are kept.");
        if !dialog.table.packages.is_empty() { ui.weak(format!("Packages: {}", dialog.table.packages.join(", "))); }
        let mut source_mode = dialog.source_mode;
        let source_response = ui.toggle_value(&mut source_mode, "Source");
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("table.source-mode", &source_response);
        if source_response.changed() {
            if source_mode {
                match dialog.table.render_source_draft_with_spec(&dialog.column_spec) { Ok(draft) => { dialog.draft = draft; dialog.source_mode = true; dialog.error = None; dialog.validation = dialog.table.source_edit(&dialog.draft).err(); }, Err(error) => dialog.error = Some(error) }
            } else if crate::table_sampling::view(ui).is_some_and(|view| !view.locked.is_empty())
                && dialog.table.render_source_draft_with_spec(&dialog.column_spec).as_ref() != Ok(&dialog.draft)
            {
                dialog.error = Some("Apply your source changes and reopen the table to refresh its sampled values.".into());
            } else {
                match dialog.table.source_edit(&dialog.draft).and_then(|_| crate::editor_features::tex_table_at(&dialog.draft, 1).ok_or("Finish the table source".into())) {
                    Ok(mut table) if table.cells.is_some() => {
                        table.source_range = dialog.table.source_range.clone();
                        table.packages.clone_from(&dialog.table.packages);
                        dialog.column_spec.clone_from(&table.column_spec);
                        dialog.table = table; dialog.source_mode = false; dialog.error = None; dialog.validation = dialog.table.grid_source_edit().err();
                    }
                    Ok(_) => dialog.error = Some("This table uses structural commands or custom columns. Keep editing its complete source here.".into()),
                    Err(error) => dialog.error = Some(error),
                }
            }
        }
    });
    if dialog.source_mode {
        if source_field(
            ui,
            "tex-table-source",
            &mut dialog.draft,
            "tex",
            height - 130.0,
            settings,
        ) {
            dialog.validation = dialog.table.source_edit(&dialog.draft).err();
            dialog.error = None;
        }
        return footer(ui, &dialog.error, dialog.validation.clone());
    }
    if crate::table_sampling::view(ui).is_some_and(|view| !view.locked.is_empty()) {
        ui.weak("Variable cells are sampled and locked");
    }
    let mut column_spec_changed = false;
    egui::CollapsingHeader::new("Table settings")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Columns");
                let spec = ui.add_enabled(
                    !crate::table_sampling::view(ui).is_some_and(|view| !view.locked.is_empty()),
                    egui::TextEdit::singleline(&mut dialog.column_spec)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("table.columns", &spec);
                column_spec_changed = spec.changed();
                spec.on_hover_text(dialog.table.environment.column_help());
            });
        });
    let mut cells_changed = false;
    if let Some(rows) = &mut dialog.table.cells {
        let count = rows.first().map_or(1, Vec::len);
        let cell_width = ((ui.available_width() - 28.0) / count as f32).max(100.0);
        let width = 28.0 + cell_width * count as f32;
        // Anchor cell editors to the table UI, not the auto IDs of the visible
        // rows. Virtualized scrolling changes which row is drawn first.
        let cells_id = ui.make_persistent_id("tex-table-cell-editors");
        egui::ScrollArea::both()
            .id_salt("tex-table-cells")
            .max_height((height - 180.0).max(70.0))
            .show_rows(ui, 74.0, rows.len(), |ui, visible| {
                ui.set_width(width);
                for r in visible {
                    let (area, _) = ui.allocate_exact_size(Vec2::new(width, 74.0), Sense::hover());
                    let number = ui.painter().layout_no_wrap(
                        (r + 1).to_string(),
                        theme::editor_font(),
                        ui.visuals().weak_text_color(),
                    );
                    ui.painter().galley(
                        area.left_top() + Vec2::new(2.0, 8.0),
                        number,
                        ui.visuals().weak_text_color(),
                    );
                    for (c, cell) in rows[r].iter_mut().enumerate() {
                        let rect = Rect::from_min_size(
                            area.min + Vec2::new(28.0 + c as f32 * cell_width, 0.0),
                            Vec2::new(cell_width, 70.0),
                        )
                        .shrink(2.0);
                        let id = cells_id.with((r, c));
                        let visual = &ui.visuals().widgets.noninteractive;
                        ui.painter().rect(
                            rect,
                            2.0,
                            visual.weak_bg_fill,
                            visual.bg_stroke,
                            StrokeKind::Inside,
                        );
                        ui.scope_builder(egui::UiBuilder::new().id_salt(id).max_rect(rect), |ui| {
                            ui.set_clip_rect(ui.clip_rect().intersect(rect));
                            let mut layouter =
                                |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width: f32| {
                                    let mut job = code_editor::highlight_job(
                                        ui,
                                        id,
                                        buffer.as_str(),
                                        "tex",
                                        settings,
                                    );
                                    job.wrap.max_width = width;
                                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                                };
                            egui::ScrollArea::vertical()
                                .id_salt(("tex-cell-scroll", r, c))
                                .max_height(rect.height())
                                .show(ui, |ui| {
                                    if table_samples::locked_cell(ui, (r, c), cell).is_some() {
                                        return;
                                    }
                                    let response = ui.add(
                                        egui::TextEdit::multiline(cell)
                                            .id(id)
                                            .code_editor()
                                            .frame(egui::Frame::NONE)
                                            .font(theme::editor_font())
                                            .desired_width(rect.width())
                                            .desired_rows(2)
                                            .layouter(&mut layouter),
                                    );
                                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                    crate::desktop_test::observe(
                                        &format!("table.cell.{r}.{c}"),
                                        &response,
                                    );
                                    if response.has_focus() {
                                        ui.painter().rect_stroke(
                                            rect,
                                            2.0,
                                            ui.style().interact(&response).bg_stroke,
                                            StrokeKind::Inside,
                                        );
                                    }
                                    cells_changed |= response.changed();
                                });
                        });
                    }
                }
            });
    }
    refresh_tex_grid_validation(dialog, column_spec_changed || cells_changed);
    footer(ui, &dialog.error, dialog.validation.clone())
}

fn refresh_tex_grid_validation(dialog: &mut TexDialog, changed: bool) {
    // Keep validation for unfinished expressions, including while the settings
    // section is collapsed. An idle frame must not reparse the complete table.
    if !changed {
        return;
    }
    let column_error = if dialog.column_spec == dialog.table.column_spec {
        None
    } else {
        dialog.table.set_column_spec(&dialog.column_spec).err()
    };
    dialog.validation = column_error.or_else(|| dialog.table.grid_source_edit().err());
    dialog.error = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _, by},
    };
    fn key() -> DocumentKey {
        DocumentKey {
            owner: tiptoptyp_core::document::WindowSessionId::new(1),
            epoch: 1,
            revision: 2,
        }
    }
    #[test]
    fn dynamic_table_source_edits_keep_surrounding_unicode_and_reject_stale_document() {
        let source = "é #table(columns: sizes, ..rows) fin";
        let target = crate::editor_features::table_source_at(source, 4).unwrap();
        let dialog = TableEditorDialog::Source(Box::new(SourceDialog {
            draft: "table(columns: sizes, ..changed)".into(),
            target,
            key: key(),
            error: None,
            validation: None,
        }));
        let edit = prepare_source_edit(source, key(), &dialog).unwrap();
        let mut result = source.to_owned();
        result.replace_range(edit.byte_range, &edit.replacement);
        assert_eq!(result, "é #table(columns: sizes, ..changed) fin");
        let mut stale = key();
        stale.revision += 1;
        assert!(prepare_source_edit(source, stale, &dialog).is_err());
        assert!(prepare_source_edit(&source.replace("rows", "other"), key(), &dialog).is_err());
    }
    #[test]
    fn table_samples_dynamic_grid_is_read_only_and_source_is_an_explicit_choice() {
        let target = TableSource {
            kind: crate::editor_features::TableKind::Table,
            source_range: 0..14,
            source: "table(..rows)".into(),
        };
        let dialog = TableEditorDialog::Source(Box::new(SourceDialog {
            draft: target.source.clone(),
            target,
            key: key(),
            error: None,
            validation: None,
        }));
        let mut ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(620.0, 560.0))
            .build_ui_state(
                move |ui, dialog| {
                    if !ready {
                        theme::configure_test_fonts(ui.ctx());
                        ready = true;
                    }
                    crate::table_sampling::publish(
                        ui.ctx(),
                        ui.ctx().viewport_id(),
                        Arc::new(crate::table_sampling::View {
                            grid: Some(crate::table_sampling::Grid {
                                columns: 1,
                                rows: 1,
                                instances: 1,
                                cells: vec![crate::table_sampling::Cell {
                                    position: (0, 0),
                                    span: (1, 1),
                                    value: "Revenue".into(),
                                }],
                            }),
                            ..Default::default()
                        }),
                    );
                    show(ui, dialog, 590.0, 520.0, &AppSettings::default());
                },
                dialog,
            );
        harness.run();
        assert_eq!(
            harness
                .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
                .count(),
            0
        );
        harness.get_by_label("Revenue").click();
        harness.event(egui::Event::Text("overwrite".into()));
        harness.run();
        let TableEditorDialog::Source(dialog) = harness.state() else {
            unreachable!()
        };
        assert_eq!(dialog.draft, "table(..rows)");
        harness.get_by_label("Source").click();
        harness.run();
        assert_eq!(
            harness
                .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
                .count(),
            1
        );
        assert!(harness.get_by_label("Apply").rect().bottom() <= 560.0);
    }

    #[test]
    fn tex_grid_and_source_modes_share_lossless_apply_and_reject_stale_source() {
        let source =
            "Préface \\begin{tabular}{|l|r|} A & B \\\\ \\hline C & D \\\\ \\end{tabular} fin";
        let table =
            crate::editor_features::tex_table_at(source, source.find("A &").unwrap()).unwrap();
        let mut dialog = TexDialog {
            original: table.source.clone(),
            draft: table.source.clone(),
            column_spec: table.column_spec.clone(),
            source_mode: false,
            table,
            key: key(),
            error: None,
            validation: None,
        };
        dialog.table.cells.as_mut().unwrap()[0][1] = "Édité".into();
        let grid = TableEditorDialog::Tex(Box::new(dialog.clone()));
        let edit = prepare_source_edit(source, key(), &grid).unwrap();
        assert!(edit.replacement.contains("Édité"));
        assert!(edit.replacement.contains("\\hline"));
        dialog.source_mode = true;
        dialog.draft = dialog.table.render_replacement().unwrap();
        assert_eq!(
            prepare_source_edit(source, key(), &TableEditorDialog::Tex(Box::new(dialog))).unwrap(),
            edit
        );
        assert!(prepare_source_edit(&source.replace("A &", "X &"), key(), &grid).is_err());
    }
    #[test]
    fn stale_table_menu_cannot_open_another_document_with_identical_source() {
        for (source, kind) in [
            ("#table([A])", DocumentKind::Typst),
            ("#table(..cells)", DocumentKind::Typst),
            ("\\begin{tabular}{l}A\\end{tabular}", DocumentKind::Tex),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            app.document_mut().replace_loaded_unprojected(
                source.into(),
                directory.path().join("a"),
                kind,
                None,
            );
            let target = BoundTableTarget {
                key: app.document().key(),
                target: Box::new(TableTarget::at(source, 2, kind).unwrap()),
            };
            app.document_mut().replace_loaded_unprojected(
                source.into(),
                directory.path().join("b"),
                kind,
                None,
            );
            app.pending_app_popup_action =
                Some(AppPopupAction::Editor(EditorMenuAction::EditTable(target)));
            app.execute_pending_app_popup_action(&context, None);
            assert!(app.table_editor.is_none());
            assert_eq!(app.document().source(), source);
        }
    }

    #[test]
    fn package_tex_workbenches_edit_cells_and_roundtrip_through_source() {
        for (name, args) in [
            ("tblr", "[baseline=T]{colspec={Q[l]X[2,r]},hlines,vlines}"),
            ("tabular", "{lS[table-format=2.0]}"),
            ("tabulary", r"{\linewidth}{LJ}"),
            ("NiceTabular", "{lc}[hvlines]"),
        ] {
            let header = format!(r"\begin{{{name}}}{args}");
            let body = format!(r"{header}Alpha & 60\\\end{{{name}}}");
            let source = format!(
                "\\documentclass{{article}}\n\\usepackage{{tabularray,siunitx,tabulary,nicematrix}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n"
            );
            let cursor = source[..source.find(&header).unwrap()].chars().count() + 1;
            let table = crate::editor_features::tex_table_at(&source, cursor).unwrap();
            let dialog = TableEditorDialog::Tex(Box::new(TexDialog {
                draft: table.source.clone(),
                original: table.source.clone(),
                column_spec: table.column_spec.clone(),
                source_mode: false,
                validation: None,
                table,
                key: key(),
                error: None,
            }));
            let mut fonts_ready = false;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(620.0, 560.0))
                .build_ui_state(
                    move |ui, dialog| {
                        if !fonts_ready {
                            theme::configure_test_fonts(ui.ctx());
                            fonts_ready = true;
                            return;
                        }
                        show(ui, dialog, 590.0, 520.0, &AppSettings::default());
                    },
                    dialog,
                );
            harness.run();
            assert_eq!(
                harness
                    .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
                    .count(),
                2,
                "{name}"
            );
            harness
                .get(
                    by().role(egui::accesskit::Role::MultilineTextInput)
                        .value("Alpha"),
                )
                .click();
            harness.run();
            harness.event(egui::Event::Text("!".into()));
            harness.run();
            harness.get_by_label("Source").click();
            harness.run();
            let TableEditorDialog::Tex(dialog) = harness.state() else {
                unreachable!()
            };
            assert!(dialog.source_mode);
            assert!(dialog.draft.starts_with(&header));
            assert!(dialog.draft.contains('!'));
            let draft = dialog.draft.clone();
            harness.get_by_label("Source").click();
            harness.run();
            let TableEditorDialog::Tex(dialog) = harness.state() else {
                unreachable!()
            };
            assert!(!dialog.source_mode, "{:?}", dialog.error);
            assert_eq!(
                prepare_source_edit(&source, key(), harness.state())
                    .unwrap()
                    .replacement,
                draft
            );
        }
    }

    #[test]
    fn tex_imports_table_apply_is_one_undo_and_cancel_does_not_touch_preamble() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let source = "\\documentclass{article}\n\\begin{document}\nPréface α\n\\begin{tabular}{lS[table-format=2.0]}Alpha & 60\\end{tabular}\n\\end{document}\n";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            directory.path().join("test.tex"),
            DocumentKind::Tex,
            None,
        );
        let cursor = source[..source.find("Alpha").unwrap()].chars().count();
        let table = crate::editor_features::tex_table_at(source, cursor).unwrap();
        app.begin_table_target(TableTarget::Tex(table.clone()));
        app.cancel_table_editor(&context);
        assert_eq!(app.document().source(), source);
        app.begin_table_target(TableTarget::Tex(table));
        let mut dialog = app.table_editor.take().unwrap();
        let TableEditorDialog::Tex(draft) = &mut dialog else {
            unreachable!()
        };
        draft.table.cells.as_mut().unwrap()[0][0] = "Changed".into();
        let edit = prepare_source_edit(source, app.document().key(), &dialog).unwrap();
        let mut stale = app.document().key();
        stale.revision += 1;
        assert!(prepare_source_edit(source, stale, &dialog).is_err());
        app.document_mut().edit(CCursorRange::default(), |source| {
            source.replace_range(edit.byte_range, &edit.replacement)
        });
        let applied = app.document().source().clone();
        assert!(applied.contains("\\usepackage{siunitx}\n"));
        assert!(applied.contains("Changed & 60"));
        app.undo_editor(&context, false);
        assert_eq!(app.document().source(), source);
        app.undo_editor(&context, true);
        assert_eq!(app.document().source(), &applied);
    }

    #[test]
    fn tex_imports_drawn_symbols_are_atomic_and_leave_typst_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let source = "\\documentclass{article}\n\\begin{document}α \\end{document}";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            directory.path().join("test.tex"),
            DocumentKind::Tex,
            None,
        );
        let cursor = source[..source.find('α').unwrap()].chars().count() + 1;
        app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(cursor)));
        app.insert_generated_symbol(
            &context,
            &crate::handwriting::Insertion {
                text: "\\upalpha".into(),
                package: Some("upgreek".into()),
            },
        );
        let applied = app.document().source().clone();
        assert!(applied.contains("\\usepackage{upgreek}\n"));
        assert!(applied.contains("α\\upalpha \\end"));
        app.undo_editor(&context, false);
        assert_eq!(app.document().source(), source);
        app.undo_editor(&context, true);
        assert_eq!(app.document().source(), &applied);
        app.document_mut().replace_loaded_unprojected(
            "$α$".into(),
            directory.path().join("test.typ"),
            DocumentKind::Typst,
            None,
        );
        app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(2)));
        app.insert_generated_symbol(
            &context,
            &crate::handwriting::Insertion {
                text: "beta".into(),
                package: None,
            },
        );
        assert_eq!(app.document().source(), "$αbeta$");
    }

    #[test]
    fn large_tex_grid_renders_visible_rows_and_keeps_footer_available() {
        let source = format!(
            "\\begin{{tabular}}{{ll}}{}\\end{{tabular}}",
            "A & B \\\\\n".repeat(400)
        );
        let table = crate::editor_features::tex_table_at(&source, 2).unwrap();
        let dialog = TableEditorDialog::Tex(Box::new(TexDialog {
            draft: table.source.clone(),
            original: table.source.clone(),
            column_spec: table.column_spec.clone(),
            source_mode: false,
            validation: None,
            table,
            key: key(),
            error: None,
        }));
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(620.0, 560.0))
            .build_ui_state(
                move |ui, dialog| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    show(ui, dialog, 590.0, 520.0, &AppSettings::default());
                },
                dialog,
            );
        harness.run();
        let cells = harness
            .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
            .count();
        assert!(
            cells > 0 && cells < 30,
            "only visible rows should be rendered, saw {cells}"
        );
        assert!(harness.get_by_label("Apply").rect().bottom() <= 560.0);
        assert!(harness.get_by_label("Cancel").rect().bottom() <= 560.0);
    }

    #[test]
    fn tex_grid_validation_is_retained_until_a_draft_changes() {
        let table =
            crate::editor_features::tex_table_at("\\begin{tabular}{ll}A & B\\end{tabular}", 2)
                .unwrap();
        let mut dialog = TexDialog {
            draft: table.source.clone(),
            original: table.source.clone(),
            column_spec: "l>{unfinished".into(),
            source_mode: false,
            validation: None,
            table,
            key: key(),
            error: None,
        };
        refresh_tex_grid_validation(&mut dialog, true);
        let invalid = dialog.validation.clone();
        assert!(invalid.is_some());
        // A new draft value has no effect until its edit is observed. This
        // proves idle frames retain the cached result rather than reparsing.
        dialog.column_spec = "lr".into();
        for _ in 0..128 {
            refresh_tex_grid_validation(&mut dialog, false);
            assert_eq!(dialog.validation, invalid);
            assert_eq!(dialog.table.column_spec, "ll");
        }
        refresh_tex_grid_validation(&mut dialog, true);
        assert!(dialog.validation.is_none());
        assert_eq!(dialog.table.column_spec, "lr");
        dialog.table.cells.as_mut().unwrap()[0][0] = "X & Y".into();
        refresh_tex_grid_validation(&mut dialog, true);
        assert!(dialog.validation.is_some());
    }

    #[test]
    fn scrolling_tex_rows_keeps_the_same_cell_focused_and_receiving_text() {
        let source = format!(
            "\\begin{{tabular}}{{ll}}{}\\end{{tabular}}",
            (0..40)
                .map(|row| format!("row-{row} & other-{row} \\\\\n"))
                .collect::<String>()
        );
        let table = crate::editor_features::tex_table_at(&source, 2).unwrap();
        let dialog = TableEditorDialog::Tex(Box::new(TexDialog {
            draft: table.source.clone(),
            original: table.source.clone(),
            column_spec: table.column_spec.clone(),
            source_mode: false,
            validation: None,
            table,
            key: key(),
            error: None,
        }));
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(620.0, 700.0))
            .build_ui_state(
                move |ui, dialog| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    show(ui, dialog, 590.0, 660.0, &AppSettings::default());
                },
                dialog,
            );
        harness.run();
        harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("row-4"),
            )
            .click();
        harness.run();
        let focused = harness.ctx.memory(|memory| memory.focused()).unwrap();
        let first_rendered_row = |harness: &Harness<'_, TableEditorDialog>| {
            harness
                .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
                .filter_map(|node| {
                    node.accesskit_node()
                        .value()?
                        .strip_prefix("row-")?
                        .parse::<usize>()
                        .ok()
                })
                .min()
                .unwrap()
        };
        let first_before = first_rendered_row(&harness);
        let cell = harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("row-4"),
            )
            .rect();
        // Wheel over the grid's row gutter, outside the cell's own scrollbar.
        harness.input_mut().events.extend([
            egui::Event::PointerMoved(Pos2::new(cell.left() - 8.0, cell.center().y)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -164.0),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ]);
        harness.run_steps(8);
        assert!(
            first_rendered_row(&harness) > first_before,
            "the wheel must change the first rendered row"
        );
        assert_eq!(harness.ctx.memory(|memory| memory.focused()), Some(focused));
        harness
            .input_mut()
            .events
            .push(egui::Event::Text("!".into()));
        harness.run();
        let TableEditorDialog::Tex(dialog) = harness.state() else {
            unreachable!()
        };
        let rows = dialog.table.cells.as_ref().unwrap();
        assert!(
            rows[4][0].contains('!'),
            "typing must reach the original focused cell"
        );
        assert_eq!(rows[3][0], "row-3");
        assert_eq!(rows[5][0], "row-5");
    }

    #[test]
    fn source_workbench_replaces_invalid_draft_with_chunked_native_text() {
        let original = "#table(columns: widths, ..rows)";
        let target = crate::editor_features::table_source_at(original, 2).unwrap();
        let dialog = TableEditorDialog::Source(Box::new(SourceDialog {
            draft: target.source.clone(),
            target,
            key: key(),
            error: None,
            validation: None,
        }));
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(620.0, 560.0))
            .build_ui_state(
                move |ui, state| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    crate::child_view::deliver_queued_edit_events(ui.ctx(), &mut state.2);
                    if let Some(action) =
                        show(ui, &mut state.0, 590.0, 520.0, &AppSettings::default())
                    {
                        state.1 = Some(action);
                    }
                },
                (
                    dialog,
                    None::<TableEditorUiAction>,
                    Vec::<egui::Event>::new(),
                ),
            );
        harness.run();
        harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value(&original[1..]),
            )
            .click();
        harness.run();

        let select_all = || egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        };
        let assert_draft = |harness: &Harness<
            '_,
            (
                TableEditorDialog,
                Option<TableEditorUiAction>,
                Vec<egui::Event>,
            ),
        >,
                            expected: &str,
                            valid: bool| {
            let TableEditorDialog::Source(dialog) = &harness.state().0 else {
                unreachable!()
            };
            assert_eq!(dialog.draft, expected);
            assert_eq!(dialog.validation.is_none(), valid);
            assert_eq!(
                !harness.get_by_label("Apply").accesskit_node().is_disabled(),
                valid,
            );
            assert!(
                harness.state().2.is_empty(),
                "queued edit must be delivered once"
            );
        };

        // Native menu commands may arrive in the same paint as the first
        // Quartz text chunk. They must run before that text is inserted.
        let incomplete = "table(columns: widths, ..rows";
        harness.state_mut().2.push(select_all());
        harness.event(egui::Event::Text(incomplete[..16].into()));
        harness.run();
        harness.event(egui::Event::Text(incomplete[16..].into()));
        harness.run();
        assert_draft(&harness, incomplete, false);

        let complete = "table(columns: widths, fill: blue, ..rows)";
        harness.state_mut().2.push(select_all());
        harness.event(egui::Event::Text(complete[..16].into()));
        harness.run();
        harness.event(egui::Event::Text(complete[16..32].into()));
        harness.run();
        harness.event(egui::Event::Text(complete[32..].into()));
        harness.run();
        assert_draft(&harness, complete, true);
        harness.get_by_label("Apply").click();
        harness.run();
        assert_eq!(harness.state().1, Some(TableEditorUiAction::Apply));
        let edit = prepare_source_edit(original, key(), &harness.state().0).unwrap();
        assert_eq!(edit.replacement, complete);
        let mut result = original.to_owned();
        result.replace_range(edit.byte_range, &edit.replacement);
        assert_eq!(result, format!("#{complete}"));
    }

    #[test]
    fn source_workbench_is_editable_and_apply_cancel_visible_at_minimum_size() {
        let target = crate::editor_features::table_source_at("#table(..cells)", 2).unwrap();
        let dialog = TableEditorDialog::Source(Box::new(SourceDialog {
            draft: target.source.clone(),
            target,
            key: key(),
            error: None,
            validation: None,
        }));
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(620.0, 560.0))
            .build_ui_state(
                move |ui, state| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    if let Some(action) =
                        show(ui, &mut state.0, 590.0, 520.0, &AppSettings::default())
                    {
                        state.1 = Some(action);
                    }
                },
                (dialog, None::<TableEditorUiAction>),
            );
        harness.run();
        harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("table(..cells)"),
            )
            .type_text(" ");
        harness.run();
        harness.get_by_label("Cancel").click();
        harness.run();
        assert_eq!(harness.state().1, Some(TableEditorUiAction::Cancel));
    }
}
