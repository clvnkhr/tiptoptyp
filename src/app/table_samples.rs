//! A workbench owns one finite sample request, bound to its locked source buffer.
use super::*;
use crate::table_sampling::{Engine, Request, View};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(super) struct Job {
    started: bool,
    cancel: Arc<AtomicBool>,
    worker: crate::worker::LatestJob<crate::table_sampling::Grid>,
    view: View,
}
impl Job {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspect(&self) -> serde_json::Value {
        serde_json::json!({"pending":self.view.pending,"failed":self.view.error.is_some(),
        "locked":self.view.locked.keys().collect::<Vec<_>>(),
        "grid":self.view.grid.as_ref().map(|grid| serde_json::json!({
            "columns":grid.columns,"rows":grid.rows,
            "values":grid.cells.iter().map(|cell| serde_json::json!({"position":cell.position,"fingerprint":crate::desktop_test::fingerprint(&cell.value)})).collect::<Vec<_>>()
        }))})
    }
    pub(super) fn reset(&mut self) {
        self.cancel.store(true, Ordering::Release);
        *self = Self::default();
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl EditorApp {
    pub(super) fn prepare_table_samples(&mut self, context: &egui::Context) {
        let viewport = scoped_child_viewport_id(context, "tiptoptyp-table-editor");
        if !self.table_samples.started {
            self.table_samples.started = true;
            let mut locked = BTreeMap::new();
            let (range, call, engine, program) = match self.table_editor.as_ref() {
                Some(TableEditorDialog::Typst(dialog)) if dialog.insertion_prefix.is_none() => {
                    for (r, row) in dialog.table.cells.iter().enumerate() {
                        for (c, cell) in row.iter().enumerate() {
                            if crate::table_sampling::computed_markup(cell) {
                                locked.insert((r, c), cell.clone());
                            }
                        }
                    }
                    if locked.is_empty() {
                        return;
                    }
                    (
                        dialog.table.source_range.clone(),
                        dialog.original_call.clone(),
                        Engine::Typst {
                            command: self.settings.typst.command.clone(),
                            fonts: self.font_catalog.workspace_directories().to_vec(),
                            kind: dialog.table.kind.callee().into(),
                        },
                        self.typst_tool.program.clone(),
                    )
                }
                Some(TableEditorDialog::Source(dialog)) => (
                    dialog.target.source_range.clone(),
                    dialog.target.source.clone(),
                    Engine::Typst {
                        command: self.settings.typst.command.clone(),
                        fonts: self.font_catalog.workspace_directories().to_vec(),
                        kind: dialog.target.kind.callee().into(),
                    },
                    self.typst_tool.program.clone(),
                ),
                Some(TableEditorDialog::Tex(dialog)) if !dialog.source_mode => {
                    if let Some(rows) = &dialog.table.cells {
                        for (r, row) in rows.iter().enumerate() {
                            for (c, cell) in row.iter().enumerate() {
                                if tex_computed_markup(cell) {
                                    locked.insert((r, c), cell.clone());
                                }
                            }
                        }
                    }
                    if locked.is_empty() {
                        return;
                    }
                    let call = match crate::table_sampling::tex_instrument(&dialog.table, &locked) {
                        Ok(call) => call,
                        Err(error) => {
                            self.table_samples.view.locked = locked;
                            self.table_samples.view.error = Some(error);
                            crate::table_sampling::publish(
                                context,
                                viewport,
                                Arc::new(self.table_samples.view.clone()),
                            );
                            return;
                        }
                    };
                    let standard = self.settings.tex.build_engine
                        != crate::tex::settings::BuildEngine::Tectonic;
                    let (program, command) = if standard {
                        (
                            self.tex_tools
                                .distribution(self.settings.tex.build_engine)
                                .map(Path::to_owned)
                                .unwrap_or_default(),
                            crate::tool_command::CommandCustomization::default(),
                        )
                    } else {
                        (
                            self.tex_tools.tectonic.program.clone(),
                            self.settings.tex.tectonic.command.clone(),
                        )
                    };
                    (
                        dialog.table.source_range.clone(),
                        call,
                        Engine::Tex {
                            command,
                            standard,
                            only_cached: self.settings.tex.only_cached,
                            cells: locked.clone(),
                            columns: dialog.table.columns().unwrap_or(1),
                            rows: dialog.table.cells.as_ref().map_or(0, Vec::len),
                        },
                        program,
                    )
                }
                _ => return,
            };
            let root = self.project_root();
            let path = self.document().path();
            let source_dir = path
                .as_deref()
                .and_then(Path::parent)
                .unwrap_or(&root)
                .to_owned();
            let name = path
                .as_deref()
                .and_then(Path::file_name)
                .and_then(|x| x.to_str())
                .unwrap_or(if self.document().kind() == DocumentKind::Tex {
                    "table-sample.tex"
                } else {
                    "table-sample.typ"
                })
                .to_owned();
            let Some(range) = char_range_to_byte_range(self.document().source(), range) else {
                return;
            };
            self.table_samples.view = View {
                locked,
                pending: true,
                ..Default::default()
            };
            let request = Request {
                root,
                source_dir,
                name,
                source: self.document().source().into(),
                range,
                call,
                program,
                engine,
            };
            let cancel = self.table_samples.cancel.clone();
            if let Err(error) =
                self.table_samples
                    .worker
                    .start_and_repaint("table-values", context, move || {
                        crate::table_sampling::sample(request, &cancel)
                    })
            {
                self.table_samples.view.pending = false;
                self.table_samples.view.error = Some(error);
            }
            crate::table_sampling::publish(
                context,
                viewport,
                Arc::new(self.table_samples.view.clone()),
            );
        }
        match self.table_samples.worker.poll() {
            crate::worker::LatestJobPoll::Ready(grid) => {
                self.table_samples.view.grid = Some(grid);
                self.table_samples.view.pending = false;
            }
            crate::worker::LatestJobPoll::Failed(error) => {
                self.table_samples.view.error = Some(error);
                self.table_samples.view.pending = false;
            }
            _ => return,
        }
        crate::table_sampling::publish(
            context,
            viewport,
            Arc::new(self.table_samples.view.clone()),
        );
        context.request_repaint_of(viewport);
    }
    pub(super) fn validate_table_sample_locks(&self) -> Result<(), String> {
        if let Some(TableEditorDialog::Typst(dialog)) = &self.table_editor {
            for (&(r, c), source) in &self.table_samples.view.locked {
                if dialog.table.cells.get(r).and_then(|row| row.get(c)) != Some(source) {
                    return Err(
                        "A sampled cell is locked. Change its variable in the code editor instead."
                            .into(),
                    );
                }
            }
        }
        if let Some(TableEditorDialog::Tex(dialog)) = &self.table_editor
            && !dialog.source_mode
        {
            for (&(r, c), source) in &self.table_samples.view.locked {
                if dialog
                    .table
                    .cells
                    .as_ref()
                    .and_then(|rows| rows.get(r))
                    .and_then(|row| row.get(c))
                    != Some(source)
                {
                    return Err(
                        "A sampled cell is locked. Change its macro in the code editor instead."
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
}
// A macro may come from an imported preamble or package, so source-local
// declarations cannot safely decide which cells are computed. Named controls
// are conservatively sampled; escaped punctuation and literal \verb stay text.
fn tex_computed_markup(cell: &str) -> bool {
    let bytes = cell.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i] == b'\\' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            if start == i {
                i = (i + 1).min(bytes.len());
                continue;
            }
            if &cell[start..i] != "verb" {
                return true;
            }
            if bytes.get(i) == Some(&b'*') {
                i += 1;
            }
            if let Some(&delimiter) = bytes.get(i) {
                i += 1;
                while i < bytes.len() && bytes[i] != delimiter && bytes[i] != b'\n' {
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
            }
        } else {
            i += 1;
        }
    }
    false
}

pub(super) fn locked_cell(
    ui: &mut egui::Ui,
    position: crate::editor_features::CellPosition,
    source: &str,
) -> Option<egui::Response> {
    let view = crate::table_sampling::view(ui)?;
    if !view.locked.contains_key(&position) {
        return None;
    }
    let text = view.locked_value(position).unwrap_or(if view.pending {
        "…"
    } else {
        "Sample unavailable"
    });
    let response = ui.add(egui::Label::new(text).selectable(true).wrap());
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(
        &format!("table.locked.{}.{}", position.0, position.1),
        &response,
    );
    if response.clicked() {
        ui.memory_mut(|memory| {
            if let Some(focused) = memory.focused() {
                memory.surrender_focus(focused);
            }
        });
    }
    Some(response.on_hover_ui(|ui| {
        ui.strong("Sampled value · locked");
        ui.label("Change the variable in your document to update this cell.");
        ui.monospace(source);
        if let Some(error) = &view.error {
            ui.label(error);
        }
    }))
}
pub(super) fn sampled_grid(ui: &mut egui::Ui, height: f32) {
    let Some(view) = crate::table_sampling::view(ui) else {
        return;
    };
    ui.weak("Sampled values · locked").on_hover_text("These are values from the document as opened. Variables, generated cells and expressions are preserved. Use Source for deliberate expression changes.");
    let Some(grid) = &view.grid else {
        if view.pending {
            ui.label("Sampling table values…");
        } else if let Some(error) = &view.error {
            ui.label(error);
        }
        return;
    };
    if grid.instances > 1 {
        ui.small(format!("First of {} table instances", grid.instances));
    }
    let width = (ui.available_width() / grid.columns.max(1) as f32).max(100.0);
    egui::ScrollArea::both()
        .id_salt("sampled-table-grid")
        .max_height((height - 110.0).max(80.0))
        .show_rows(ui, 66.0, grid.rows, |ui, rows| {
            let start = ui.cursor().min;
            let first = rows.start;
            ui.allocate_space(Vec2::new(
                width * grid.columns as f32,
                66.0 * rows.len() as f32,
            ));
            for cell in grid
                .cells
                .iter()
                .filter(|cell| cell.position.0 + cell.span.0 > first && cell.position.0 < rows.end)
            {
                let rect = Rect::from_min_size(
                    start
                        + Vec2::new(
                            cell.position.1 as f32 * width,
                            (cell.position.0 as f32 - first as f32) * 66.0,
                        ),
                    Vec2::new(width * cell.span.1 as f32, 66.0 * cell.span.0 as f32),
                )
                .shrink(2.0);
                if !ui.is_rect_visible(rect) {
                    continue;
                }

                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                    StrokeKind::Inside,
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(6.0)), |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(rect));
                    let response = ui.add(egui::Label::new(&cell.value).selectable(true).wrap());
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(
                        &format!("table.sample.{}.{}", cell.position.0, cell.position.1),
                        &response,
                    );
                    let _ = response;
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_samples_apply_refuses_programmatic_overwrite_of_computed_cells() {
        let dir = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, dir.path().into());
        let source = "#let price = 60\n#table(columns: 2, [Revenue], [#price])";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            dir.path().join("sample.typ"),
            DocumentKind::Typst,
            None,
        );
        app.begin_table_target(
            TableTarget::at(source, source.find("table(").unwrap(), DocumentKind::Typst).unwrap(),
        );
        app.table_samples
            .view
            .locked
            .insert((0, 1), "#price".into());
        assert!(app.validate_table_sample_locks().is_ok());
        let Some(TableEditorDialog::Typst(dialog)) = &mut app.table_editor else {
            unreachable!()
        };
        dialog.table.cells[0][1] = "60".into();
        assert!(app.validate_table_sample_locks().is_err());
        assert_eq!(app.document().source(), source);
    }
    #[test]
    fn table_samples_inherited_computed_columns_use_read_only_grid() {
        let dir = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, dir.path().into());
        let source = "#set table(columns: 2)\n#let price = 60\n#table([Revenue], [#price])";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            dir.path().join("sample.typ"),
            DocumentKind::Typst,
            None,
        );
        app.begin_table_target(
            TableTarget::at(source, source.find("table([").unwrap(), DocumentKind::Typst).unwrap(),
        );
        assert!(matches!(
            app.table_editor,
            Some(TableEditorDialog::Source(_))
        ));
    }
    #[test]
    fn table_samples_macro_detection_handles_imports_comments_literals_and_escapes() {
        assert!(tex_computed_markup(r"Price: \price"));
        assert!(tex_computed_markup(r"\macrofromanotherfile"));
        assert!(tex_computed_markup(r"\textbf{\price}"));
        assert!(!tex_computed_markup(r"\\price"));
        assert!(!tex_computed_markup(r"\% \$ \&"));
        assert!(!tex_computed_markup("Plain % \\price\ntext"));
        assert!(!tex_computed_markup(r"\verb|\price|"));
        assert!(!tex_computed_markup(r"\verb*+\price+"));
        assert!(tex_computed_markup(r"\verb|literal| \price"));
    }
}
