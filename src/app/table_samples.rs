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
    _source: &str,
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
    let response = ui
        .push_id(("table-locked-value", position), |ui| {
            ui.add(egui::Label::new(text).selectable(true).wrap())
        })
        .inner;
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
    Some(response)
}

fn sampled_visible_rows(viewport: Rect, rows: usize) -> std::ops::Range<usize> {
    use table_editor::{CELL_HEIGHT, HEADER_HEIGHT};
    let first = ((viewport.top() - HEADER_HEIGHT) / CELL_HEIGHT)
        .floor()
        .max(0.0) as usize;
    let last = ((viewport.bottom() - HEADER_HEIGHT) / CELL_HEIGHT)
        .ceil()
        .max(0.0) as usize;
    first.min(rows)..last.saturating_add(1).min(rows)
}

fn sampled_cell_intersects_rows(
    cell: &crate::table_sampling::Cell,
    rows: &std::ops::Range<usize>,
) -> bool {
    cell.position.0.saturating_add(cell.span.0) > rows.start && cell.position.0 < rows.end
}

fn sampled_cell_rect(origin: Pos2, cell: &crate::table_sampling::Cell, width: f32) -> Rect {
    use table_editor::{CELL_HEIGHT, HEADER_HEIGHT, ROW_HEADER};
    Rect::from_min_size(
        origin
            + Vec2::new(
                ROW_HEADER + cell.position.1 as f32 * width,
                HEADER_HEIGHT + cell.position.0 as f32 * CELL_HEIGHT,
            ),
        Vec2::new(width * cell.span.1 as f32, CELL_HEIGHT * cell.span.0 as f32),
    )
}

pub(super) fn sampled_grid(ui: &mut egui::Ui, height: f32) {
    use table_editor::{CELL_HEIGHT, HEADER_HEIGHT, ROW_HEADER};
    let Some(view) = crate::table_sampling::view(ui) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.weak("Sampled values · read-only");
        if let Some(grid) = &view.grid
            && grid.instances > 1
        {
            ui.weak(format!("1 / {}", grid.instances));
        }
    });
    let Some(grid) = &view.grid else {
        if view.pending {
            ui.label("Sampling table values…");
        } else if let Some(error) = &view.error {
            egui::ScrollArea::vertical()
                .id_salt("sampled-table-error")
                .max_height(height.min(ui.available_height()).max(0.0))
                .min_scrolled_height(0.0)
                .show(ui, |ui| {
                    ui.label(error);
                });
        }
        return;
    };
    let columns = grid.columns.max(1);
    let width = ((ui.available_width() - ROW_HEADER - 12.0) / columns.min(5) as f32).max(128.0);
    let content_width = ROW_HEADER + width * columns as f32;
    egui::ScrollArea::both()
        .id_salt("sampled-table-grid")
        .max_height(height.min(ui.available_height()).max(0.0))
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let (_, area) = ui.allocate_space(Vec2::new(
                content_width,
                HEADER_HEIGHT + CELL_HEIGHT * grid.rows as f32,
            ));
            let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
            let fill = ui.visuals().faint_bg_color;
            let header_fill = ui.visuals().extreme_bg_color;
            let small = egui::TextStyle::Small.resolve(ui.style());
            let header = Rect::from_min_size(area.min, Vec2::new(content_width, HEADER_HEIGHT));
            ui.painter().rect_filled(header, 0.0, header_fill);
            for column in 0..columns {
                let rect = Rect::from_min_size(
                    area.min + Vec2::new(ROW_HEADER + column as f32 * width, 0.0),
                    Vec2::new(width, HEADER_HEIGHT),
                );
                if ui.is_rect_visible(rect) {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        table_editor::column_name(column),
                        small.clone(),
                        ui.visuals().weak_text_color(),
                    );
                    ui.painter()
                        .line_segment([rect.left_top(), rect.left_bottom()], stroke);
                }
            }
            ui.painter()
                .line_segment([header.left_top(), header.right_top()], stroke);
            let rows = sampled_visible_rows(viewport, grid.rows);
            for row in rows.clone() {
                let rect = Rect::from_min_size(
                    area.min + Vec2::new(0.0, HEADER_HEIGHT + row as f32 * CELL_HEIGHT),
                    Vec2::new(ROW_HEADER, CELL_HEIGHT),
                );
                ui.painter().rect_filled(rect, 0.0, header_fill);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    (row + 1).to_string(),
                    small.clone(),
                    ui.visuals().weak_text_color(),
                );
                ui.painter()
                    .line_segment([rect.left_top(), rect.right_top()], stroke);
                if row + 1 == grid.rows {
                    ui.painter()
                        .line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
                }
            }
            for cell in grid
                .cells
                .iter()
                .filter(|cell| sampled_cell_intersects_rows(cell, &rows))
            {
                let rect = sampled_cell_rect(area.min, cell, width);
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                ui.painter().rect_filled(rect, 0.0, fill);
                ui.painter()
                    .line_segment([rect.left_top(), rect.right_top()], stroke);
                ui.painter()
                    .line_segment([rect.left_top(), rect.left_bottom()], stroke);
                if cell.position.0.saturating_add(cell.span.0) == grid.rows {
                    ui.painter()
                        .line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
                }
                if cell.position.1.saturating_add(cell.span.1) == columns {
                    ui.painter()
                        .line_segment([rect.right_top(), rect.right_bottom()], stroke);
                }
                let inner = rect.shrink2(Vec2::new(6.0, 5.0));
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt(("sampled-table-cell", cell.position))
                        .max_rect(inner),
                    |ui| {
                        ui.set_clip_rect(ui.clip_rect().intersect(inner));
                        egui::ScrollArea::vertical()
                            .id_salt("value-scroll")
                            .max_height(inner.height())
                            .min_scrolled_height(0.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let response =
                                    ui.add(egui::Label::new(&cell.value).selectable(true).wrap());
                                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                crate::desktop_test::observe(
                                    &format!(
                                        "table.sample.{}.{}",
                                        cell.position.0, cell.position.1
                                    ),
                                    &response,
                                );
                                let _ = response;
                            });
                    },
                );
            }
            for edge in [
                [area.left_top(), area.left_bottom()],
                [area.right_top(), area.right_bottom()],
            ] {
                ui.painter().line_segment(edge, stroke);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sampled_text_frame(
        context: &egui::Context,
        text: &str,
        frame: usize,
        events: Vec<egui::Event>,
    ) -> Option<(Rect, Rect)> {
        fn text_rect(shape: &egui::Shape, text: &str) -> Option<Rect> {
            match shape {
                egui::Shape::Text(value) if value.galley.job.text == text => {
                    Some(Rect::from_min_size(value.pos, value.galley.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, text)),
                _ => None,
            }
        }
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(440.0, 280.0))),
                time: Some(frame as f64 / 60.0),
                events,
                ..Default::default()
            },
            |ui| sampled_grid(ui, ui.available_height()),
        );
        let shape = output
            .shapes
            .iter()
            .find_map(|shape| text_rect(&shape.shape, text).map(|rect| (rect, shape.clip_rect)));
        output.drop_without_applying_deltas();
        shape
    }

    #[test]
    fn table_samples_long_cell_scroll_reaches_last_line_without_changing_source() {
        let context = egui::Context::default();
        theme::configure_test_fonts(&context);
        let text = (0..40)
            .map(|line| format!("Sample line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let grid = crate::table_sampling::Grid {
            columns: 1,
            rows: 1,
            instances: 1,
            cells: vec![crate::table_sampling::Cell {
                position: (0, 0),
                span: (1, 1),
                value: text.clone(),
            }],
        };
        let source = BTreeMap::from([((0, 0), "#long_value".to_owned())]);
        crate::table_sampling::publish(
            &context,
            context.viewport_id(),
            Arc::new(View {
                locked: source.clone(),
                grid: Some(grid.clone()),
                ..Default::default()
            }),
        );
        let _ = sampled_text_frame(&context, &text, 0, Vec::new());
        let (before, clip) = sampled_text_frame(&context, &text, 1, Vec::new())
            .expect("the read-only sample must be rendered");
        assert!(before.bottom() > clip.bottom() + 100.0);
        let pointer = clip.intersect(before).center();
        let mut after = before;
        let mut after_clip = clip;
        for frame in 2..22 {
            let mut events = vec![egui::Event::PointerMoved(pointer)];
            if frame == 2 {
                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -8_000.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                });
            }
            (after, after_clip) = sampled_text_frame(&context, &text, frame, events)
                .expect("the read-only sample must remain rendered");
        }
        assert!(
            after.bottom() <= after_clip.bottom() + 1.0 && after.bottom() >= after_clip.top(),
            "the final sampled line must scroll into the cell clip: {after:?}, {after_clip:?}"
        );
        let mut unchanged = None;
        let output = context.run_ui(Default::default(), |ui| {
            unchanged = crate::table_sampling::view(ui);
        });
        output.drop_without_applying_deltas();
        let unchanged = unchanged.unwrap();
        assert_eq!(unchanged.locked, source);
        assert_eq!(unchanged.grid.as_ref(), Some(&grid));
    }

    #[test]
    fn table_samples_virtual_rows_keep_one_pitch_and_include_overlapping_spans() {
        use table_editor::{CELL_HEIGHT, HEADER_HEIGHT, ROW_HEADER};
        let origin = Pos2::new(11.0, 17.0);
        let cell = crate::table_sampling::Cell {
            position: (39, 2),
            span: (4, 2),
            value: "Spanning sample".into(),
        };
        let rect = sampled_cell_rect(origin, &cell, 128.0);
        assert_eq!(rect.left(), origin.x + ROW_HEADER + 256.0);
        assert_eq!(rect.top(), origin.y + HEADER_HEIGHT + 39.0 * CELL_HEIGHT);
        assert_eq!(rect.height(), 4.0 * CELL_HEIGHT);
        assert_eq!(rect.width(), 256.0);
        for offset in [0.0, 7.0, 27.0] {
            let viewport = Rect::from_min_size(
                Pos2::new(0.0, HEADER_HEIGHT + 40.0 * CELL_HEIGHT + offset),
                Vec2::new(400.0, 3.0 * CELL_HEIGHT),
            );
            let rows = sampled_visible_rows(viewport, 4_096);
            assert_eq!(rows.start, 40);
            assert!(rows.len() <= 5, "only a viewport of rows is materialized");
            assert!(sampled_cell_intersects_rows(&cell, &rows));
            let on_screen = sampled_cell_rect(origin - viewport.min.to_vec2(), &cell, 128.0);
            assert_eq!(on_screen.translate(viewport.min.to_vec2()), rect);
        }
        assert!(!sampled_cell_intersects_rows(&cell, &(43..46)));
        assert!(!sampled_cell_intersects_rows(&cell, &(35..39)));
        assert!(sampled_visible_rows(Rect::EVERYTHING, 0).is_empty());
    }

    #[test]
    fn table_samples_grid_materializes_only_visible_read_only_values() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT as _, Queryable as _, by},
        };
        let view = Arc::new(View {
            grid: Some(crate::table_sampling::Grid {
                columns: 1,
                rows: 4_096,
                instances: 1,
                cells: (0..4_096)
                    .map(|row| crate::table_sampling::Cell {
                        position: (row, 0),
                        span: (1, 1),
                        value: format!("Sample row {row}"),
                    })
                    .collect(),
            }),
            ..Default::default()
        });
        let mut ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(440.0, 280.0))
            .build_ui(move |ui| {
                if !ready {
                    theme::configure_test_fonts(ui.ctx());
                    ready = true;
                }
                // Layout spacing must not become an extra pitch between grid rows.
                ui.spacing_mut().item_spacing.y = 27.0;
                crate::table_sampling::publish(ui.ctx(), ui.ctx().viewport_id(), view.clone());
                sampled_grid(ui, ui.available_height());
            });
        harness.run();
        let samples = harness
            .query_all(by().label_contains("Sample row "))
            .filter_map(|node| node.accesskit_node().value())
            .collect::<Vec<_>>();
        assert!(samples.len() >= 2 && samples.len() <= 9, "{samples:?}");
        assert_eq!(
            harness
                .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
                .count(),
            0
        );
        assert!(harness.get_by_label("Sample row 0").rect().height() < table_editor::CELL_HEIGHT);
        let first = harness.get_by_label("Sample row 0").rect().top();
        let second = harness.get_by_label("Sample row 1").rect().top();
        assert_eq!(second - first, table_editor::CELL_HEIGHT);
    }

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
