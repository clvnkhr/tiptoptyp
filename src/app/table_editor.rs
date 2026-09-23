//! A nonmodal, draft-based table workbench. Source stays locked until Apply/Cancel.
use super::*;
use crate::editor_features::{CellPosition, TableSelection};

#[derive(Debug, Clone, Default)]
pub(super) struct TableEditorState {
    selection: TableSelection,
    focus_cell: Option<CellPosition>,
    style_selection: bool,
    importing: bool,
    markdown: String,
}

const CELL_HEIGHT: f32 = 74.0;
const HEADER_HEIGHT: f32 = 28.0;
const ROW_HEADER: f32 = 38.0;

type StyleControl = (
    &'static str,
    &'static str,
    &'static [(&'static str, Option<&'static str>)],
);
const STYLE_CONTROLS: &[StyleControl] = &[
    (
        "stroke",
        "Border",
        &[
            ("Default", None),
            ("None", Some("none")),
            ("Hairline", Some("0.5pt")),
            ("Regular", Some("1pt")),
            ("Heavy", Some("2pt")),
        ],
    ),
    (
        "fill",
        "Background",
        &[
            ("Default", None),
            ("None", Some("none")),
            ("Mist", Some("rgb(\"#eef2f6\")")),
            ("Blue", Some("rgb(\"#dbeafe\")")),
            ("Mint", Some("rgb(\"#dcfce7\")")),
            ("Rose", Some("rgb(\"#fce7f3\")")),
        ],
    ),
    (
        "align",
        "Alignment",
        &[
            ("Default", None),
            ("Left", Some("left + horizon")),
            ("Center", Some("center + horizon")),
            ("Right", Some("right + horizon")),
            ("Top left", Some("left + top")),
            ("Bottom right", Some("right + bottom")),
        ],
    ),
    (
        "inset",
        "Padding",
        &[
            ("Default", None),
            ("Compact", Some("4pt")),
            ("Comfortable", Some("8pt")),
            ("Spacious", Some("12pt")),
        ],
    ),
];

fn cell_rect(
    origin: Pos2,
    (r, c): CellPosition,
    (rows, columns): (usize, usize),
    width: f32,
) -> Rect {
    Rect::from_min_size(
        origin
            + Vec2::new(
                ROW_HEADER + c as f32 * width,
                HEADER_HEIGHT + r as f32 * CELL_HEIGHT,
            ),
        Vec2::new(columns as f32 * width, rows as f32 * CELL_HEIGHT),
    )
    .shrink(3.0)
}

fn cell_id(ui: &egui::Ui, cell: CellPosition) -> egui::Id {
    viewport_scoped_id(ui.ctx(), "table-cell").with(cell)
}

pub(super) fn show_table_editor_ui(
    ui: &mut egui::Ui,
    dialog: &mut TableEditorDialog,
    available_width: f32,
    cells_height: f32,
) -> Option<TableEditorUiAction> {
    let rows = dialog.table.row_count();
    let columns = dialog.table.columns;
    dialog.ui.selection.clamp(rows, columns);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Table editor").size(20.0).strong());
        ui.label(RichText::new(format!("{rows} rows × {columns} columns")).weak());
    });
    ui.add(egui::Label::new(RichText::new("Draft changes · source is read-only until you apply or cancel. You can still scroll and copy code.").weak()).wrap());
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if crate::app::icons::action_button_enabled(ui, dialog.table.can_add_row(), "Add row").clicked() { dialog.table.add_row(); }
        if crate::app::icons::action_button_enabled(ui, dialog.table.can_add_column(), "Add column").clicked() { dialog.table.add_column(); }
        ui.menu_button("Remove…", |ui| {
            if crate::app::icons::action_button_enabled(ui, rows > 0, "Selected row").clicked() {
                dialog.table.remove_row(dialog.ui.selection.focus.0); ui.close();
            }
            if crate::app::icons::action_button_enabled(ui, columns > 1, "Last column").clicked() {
                dialog.table.remove_column(); ui.close();
            }
        });
        ui.separator();
        ui.toggle_value(&mut dialog.ui.importing, "Import Markdown");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label("Keyboard help").on_hover_text("Tab / Shift+Tab: next / previous cell\nAlt+arrows: move between cells\nAlt+Shift+arrows: extend selection\nShift+Space: select row\nCtrl+Space: select column\nCmd/Ctrl+Shift+A: select all cells\nClick row/column headers to select; Shift-click cells to extend.");
        });
    });
    dialog
        .ui
        .selection
        .clamp(dialog.table.row_count(), dialog.table.columns);
    if dialog.ui.importing {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label("Paste one Markdown table, including its header separator. Import replaces this draft, not the source.");
            egui::ScrollArea::vertical().id_salt("markdown-import-scroll").max_height(110.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut dialog.ui.markdown).code_editor().desired_rows(4).desired_width(f32::INFINITY).hint_text("| Name | Value |\n| :--- | ---: |\n| Alpha | 42 |"));
            });
            if crate::app::icons::action_button(ui, "Replace draft from Markdown").clicked() {
                match dialog.table.import_markdown(&dialog.ui.markdown) {
                    Ok(()) => { dialog.ui.selection = Default::default(); dialog.focus_first_cell = true; dialog.ui.importing = false; dialog.error = None; }
                    Err(error) => dialog.error = Some(error),
                }
            }
            ui.small("Text, emphasis and inline-code text are supported. Links, images and HTML are rejected.");
        });
        return table_footer(ui, dialog);
    }
    ui.add_space(6.0);
    let owners = dialog.table.cell_owners().unwrap_or_default();
    let cells = selected_cells(dialog, &owners);
    egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width((available_width - 22.0).max(1.0));
            ui.horizontal_wrapped(|ui| {
                ui.strong("Appearance");
                ui.selectable_value(&mut dialog.ui.style_selection, false, "Whole table");
                ui.selectable_value(&mut dialog.ui.style_selection, true, "Selected cells");
            });
            let columns = if available_width >= 760.0 { 4 } else { 2 };
            for row in STYLE_CONTROLS.chunks(columns) {
                ui.columns(columns, |uis| {
                    for (ui, &(name, label, choices)) in uis.iter_mut().zip(row) {
                        style_picker(ui, dialog, name, &cells, label, choices);
                    }
                });
            }
        });
    ui.add_space(4.0);
    let owners = dialog.table.cell_owners().unwrap_or_default();
    let selected = selected_cells(dialog, &owners);
    let focus = owners
        .get(dialog.ui.selection.focus.0)
        .and_then(|row| row.get(dialog.ui.selection.focus.1))
        .copied();
    ui.horizontal_wrapped(|ui| {
        ui.strong(if selected.len() == 1 {
            format!("Cell {}, {}", selected[0].0 + 1, selected[0].1 + 1)
        } else {
            format!("{} cells selected", selected.len())
        });
        ui.add_enabled_ui(selected.len() == 1, |ui| {
            let cell = focus.unwrap_or((0, 0));
            let (mut rows, mut columns) = dialog.table.span(cell);
            ui.label("Row span");
            let row_changed = ui
                .add(
                    egui::DragValue::new(&mut rows)
                        .range(1..=dialog.table.row_count().saturating_sub(cell.0).max(1))
                        .speed(0.1),
                )
                .changed();
            ui.label("Column span");
            let column_changed = ui
                .add(
                    egui::DragValue::new(&mut columns)
                        .range(1..=dialog.table.columns.saturating_sub(cell.1).max(1))
                        .speed(0.1),
                )
                .changed();
            if row_changed || column_changed {
                dialog.error = dialog
                    .table
                    .set_span(cell, rows, columns)
                    .err()
                    .map(|error| error.to_string());
            }
        });
        ui.add(
            egui::Label::new(RichText::new("Spans can only cover empty, unstyled cells.").weak())
                .wrap(),
        );
    });
    if dialog.focus_first_cell && dialog.table.row_count() > 0 {
        dialog.ui.focus_cell = Some((0, 0));
        dialog.focus_first_cell = false;
    }
    let owners = dialog.table.cell_owners().unwrap_or_default();
    keyboard_selection(ui, dialog, &owners);
    let width =
        ((available_width - ROW_HEADER - 18.0) / dialog.table.columns.min(5) as f32).max(128.0);
    // Reserve the footer even in a short resized window. Widgets outside the
    // visible grid are skipped, except the requested keyboard destination.
    let footer_height = if dialog.error.is_some() { 96.0 } else { 48.0 };
    let height = cells_height.min((ui.available_height() - footer_height).max(80.0));
    egui::ScrollArea::both()
        .id_salt("table-editor-cells")
        .max_height(height)
        .min_scrolled_height(80.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (_, area) = ui.allocate_space(Vec2::new(
                ROW_HEADER + dialog.table.columns as f32 * width,
                HEADER_HEIGHT + dialog.table.row_count() as f32 * CELL_HEIGHT,
            ));
            for c in 0..dialog.table.columns {
                let rect = Rect::from_min_size(
                    area.min + Vec2::new(ROW_HEADER + c as f32 * width, 0.0),
                    Vec2::new(width, HEADER_HEIGHT),
                )
                .shrink(2.0);
                if ui.is_rect_visible(rect)
                    && ui
                        .put(
                            rect,
                            egui::Button::new(format!("Column {}", c + 1)).frame(false),
                        )
                        .clicked()
                {
                    dialog
                        .ui
                        .selection
                        .select_column(c, dialog.table.row_count());
                }
            }
            for (r, row) in owners.iter().enumerate() {
                let rect = Rect::from_min_size(
                    area.min + Vec2::new(0.0, HEADER_HEIGHT + r as f32 * CELL_HEIGHT),
                    Vec2::new(ROW_HEADER, CELL_HEIGHT),
                )
                .shrink(2.0);
                if ui.is_rect_visible(rect)
                    && ui
                        .put(rect, egui::Button::new(format!("{}", r + 1)).frame(false))
                        .on_hover_text("Select row")
                        .clicked()
                {
                    dialog.ui.selection.select_row(r, dialog.table.columns);
                }
                for (c, &owner) in row.iter().enumerate() {
                    if owner != (r, c) {
                        continue;
                    }
                    let rect = cell_rect(area.min, (r, c), dialog.table.span((r, c)), width);
                    let wants_focus = dialog.ui.focus_cell == Some((r, c));
                    let id = cell_id(ui, (r, c));
                    let has_focus = ui.memory(|memory| memory.focused() == Some(id));
                    if !ui.is_rect_visible(rect) && !wants_focus && !has_focus {
                        continue;
                    }
                    let selected = selection_intersects_cell(
                        dialog.ui.selection,
                        (r, c),
                        dialog.table.span((r, c)),
                    );
                    let fill = if selected {
                        ui.visuals().selection.bg_fill.gamma_multiply(0.18)
                    } else {
                        ui.visuals().faint_bg_color
                    };
                    ui.painter().rect_filled(rect, 6.0, fill);
                    let stroke = if selected {
                        ui.visuals().selection.stroke
                    } else {
                        ui.visuals().widgets.noninteractive.bg_stroke
                    };
                    ui.painter()
                        .rect_stroke(rect, 6.0, stroke, StrokeKind::Inside);
                    let inner = rect.shrink(8.0);
                    let response = ui
                        .scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                            ui.set_clip_rect(ui.clip_rect().intersect(inner));
                            egui::ScrollArea::vertical()
                                .id_salt(("table-cell-scroll", r, c))
                                .max_height(inner.height())
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut dialog.table.cells[r][c])
                                            .id(id)
                                            .frame(egui::Frame::NONE)
                                            .desired_width(ui.available_width())
                                            .desired_rows(2),
                                    )
                                })
                                .inner
                        })
                        .inner;
                    if response.clicked() || response.gained_focus() && !wants_focus {
                        dialog
                            .ui
                            .selection
                            .select((r, c), ui.input(|input| input.modifiers.shift));
                    }
                    if wants_focus {
                        response.request_focus();
                        ui.scroll_to_rect(rect, Some(Align::Center));
                        dialog.ui.focus_cell = None;
                    }
                    if response.changed() {
                        dialog.error = None;
                    }
                }
            }
        });
    table_footer(ui, dialog)
}

fn table_footer(ui: &mut egui::Ui, dialog: &TableEditorDialog) -> Option<TableEditorUiAction> {
    ui.add_space(6.0);
    if let Some(error) = &dialog.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Static Typst markup · Tab to move between cells").weak());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if crate::app::icons::action_button_enabled(ui, !dialog.ui.importing, "Apply").clicked()
            {
                action = Some(TableEditorUiAction::Apply);
            }
            if crate::app::icons::action_button(ui, "Cancel").clicked() {
                action = Some(TableEditorUiAction::Cancel);
            }
        });
    });
    action
}

fn selection_intersects_cell(
    selection: TableSelection,
    cell: CellPosition,
    span: (usize, usize),
) -> bool {
    if selection.contains(cell) {
        return true;
    }
    let end = (cell.0 + span.0 - 1, cell.1 + span.1 - 1);
    cell.0 <= selection.anchor.0.max(selection.focus.0)
        && end.0 >= selection.anchor.0.min(selection.focus.0)
        && cell.1 <= selection.anchor.1.max(selection.focus.1)
        && end.1 >= selection.anchor.1.min(selection.focus.1)
}

fn selected_cells(dialog: &TableEditorDialog, owners: &[Vec<CellPosition>]) -> Vec<CellPosition> {
    owners
        .iter()
        .enumerate()
        .flat_map(|(r, row)| {
            row.iter().enumerate().filter_map(move |(c, &owner)| {
                (owner == (r, c)
                    && selection_intersects_cell(
                        dialog.ui.selection,
                        owner,
                        dialog.table.span(owner),
                    ))
                .then_some(owner)
            })
        })
        .collect()
}

fn style_picker(
    ui: &mut egui::Ui,
    dialog: &mut TableEditorDialog,
    name: &str,
    cells: &[CellPosition],
    label: &str,
    choices: &[(&str, Option<&str>)],
) {
    let target = if dialog.ui.style_selection {
        cells.first().copied()
    } else {
        None
    };
    let value = dialog.table.style(target, name);
    let mixed = dialog.ui.style_selection
        && cells
            .iter()
            .any(|cell| dialog.table.style(Some(*cell), name) != value);
    let current = if mixed {
        "Mixed"
    } else {
        choices
            .iter()
            .find(|(_, candidate)| *candidate == value)
            .map_or("Custom (kept)", |(label, _)| label)
    };
    ui.label(label);
    let mut change = None;
    egui::ComboBox::from_id_salt(("table-style", name))
        .selected_text(current)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for &(label, choice) in choices {
                if ui
                    .selectable_label(!mixed && value == choice, label)
                    .clicked()
                {
                    change = Some(choice);
                }
            }
        });
    if let Some(value) = change {
        if dialog.ui.style_selection {
            for &cell in cells {
                dialog.table.set_style(Some(cell), name, value);
            }
        } else {
            dialog.table.set_style(None, name, value);
        }
    }
}

fn keyboard_selection(
    ui: &mut egui::Ui,
    dialog: &mut TableEditorDialog,
    owners: &[Vec<CellPosition>],
) {
    let anchors: Vec<_> = owners
        .iter()
        .enumerate()
        .flat_map(|(r, row)| {
            row.iter()
                .enumerate()
                .filter_map(move |(c, &owner)| (owner == (r, c)).then_some(owner))
        })
        .collect();
    let focused = ui.memory(|memory| memory.focused());
    let Some(index) = anchors
        .iter()
        .position(|&cell| focused == Some(cell_id(ui, cell)))
    else {
        return;
    };
    let current = anchors[index];
    let modifiers = ui.input(|input| input.modifiers);
    let mut destination = None;
    ui.input_mut(|input| {
        if input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::A) {
            dialog.ui.selection.anchor = (0, 0);
            dialog.ui.selection.focus = (dialog.table.row_count() - 1, dialog.table.columns - 1);
        } else if input.consume_key(Modifiers::SHIFT, egui::Key::Space) {
            dialog
                .ui
                .selection
                .select_row(current.0, dialog.table.columns);
        } else if input.consume_key(Modifiers::CTRL, egui::Key::Space) {
            dialog
                .ui
                .selection
                .select_column(current.1, dialog.table.row_count());
        } else if input.consume_key(Modifiers::SHIFT, egui::Key::Tab) {
            destination = Some(anchors[(index + anchors.len() - 1) % anchors.len()]);
        } else if input.consume_key(Modifiers::NONE, egui::Key::Tab) {
            destination = Some(anchors[(index + 1) % anchors.len()]);
        } else if modifiers.alt {
            let start = dialog.ui.selection.focus;
            let span = dialog.table.span(owners[start.0][start.1]);
            for (key, row, column) in [
                (egui::Key::ArrowUp, start.0.saturating_sub(1), start.1),
                (egui::Key::ArrowDown, start.0 + span.0, start.1),
                (egui::Key::ArrowLeft, start.0, start.1.saturating_sub(1)),
                (egui::Key::ArrowRight, start.0, start.1 + span.1),
            ] {
                if input.consume_key(modifiers, key) {
                    destination = owners.get(row).and_then(|row| row.get(column)).copied();
                }
            }
        }
    });
    if let Some(cell) = destination {
        dialog
            .ui
            .selection
            .select(cell, modifiers.alt && modifiers.shift);
        dialog.ui.focus_cell = Some(cell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(source: &str) -> TableEditorDialog {
        let table = editable_table_at(source, 2).unwrap();
        TableEditorDialog {
            original_call: char_range_slice(source, table.source_range.clone())
                .unwrap()
                .into(),
            table,
            insertion_prefix: None,
            document_key: DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 0, 0),
            focus_first_cell: true,
            ui: Default::default(),
            error: None,
        }
    }

    fn key(key: egui::Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn table_keyboard_moves_between_anchors_and_extends_selection() {
        let context = egui::Context::default();
        let mut dialog =
            dialog("#table(columns: 3, table.cell(colspan: 2)[A], [B], [C], [D], [E])");
        let frame = |dialog: &mut TableEditorDialog, mut events: Vec<egui::Event>| {
            let modifiers = events
                .iter()
                .find_map(|event| match event {
                    egui::Event::Key { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or_default();
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(940.0, 700.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        show_table_editor_ui(ui, dialog, 920.0, 680.0);
                    },
                )
                .drop_without_applying_deltas();
        };
        frame(&mut dialog, vec![]);
        frame(&mut dialog, vec![key(egui::Key::Tab, Modifiers::NONE)]);
        assert_eq!(dialog.ui.selection.focus, (0, 2));
        frame(
            &mut dialog,
            vec![key(egui::Key::ArrowDown, Modifiers::ALT | Modifiers::SHIFT)],
        );
        assert_eq!(dialog.ui.selection.anchor, (0, 2));
        assert_eq!(dialog.ui.selection.focus, (1, 2));
        frame(&mut dialog, vec![key(egui::Key::Space, Modifiers::SHIFT)]);
        assert_eq!(dialog.ui.selection.anchor, (1, 0));
        assert_eq!(dialog.ui.selection.focus, (1, 2));
        frame(
            &mut dialog,
            vec![key(egui::Key::A, Modifiers::COMMAND | Modifiers::SHIFT)],
        );
        assert_eq!(dialog.ui.selection.anchor, (0, 0));
        assert_eq!(
            selected_cells(&dialog, &dialog.table.cell_owners().unwrap()).len(),
            5
        );
    }

    #[test]
    fn table_minimum_size_keeps_apply_and_cancel_visible() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT as _, Queryable as _},
        };
        for importing in [false, true] {
            let mut dialog = dialog("#table(columns: 2, [A], [B], [C], [D])");
            dialog.table.cells[0][0] = "Long cell\n\n".repeat(80);
            dialog.ui.importing = importing;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(600.0, 520.0))
                .build_ui_state(
                    |ui, dialog| {
                        show_table_editor_ui(ui, dialog, 580.0, 500.0);
                    },
                    dialog,
                );
            harness.run();
            for label in ["Apply", "Cancel"] {
                let bounds = harness
                    .get_by_label(label)
                    .accesskit_node()
                    .bounding_box()
                    .unwrap();
                assert!(
                    bounds.y1 <= 520.0 && bounds.x1 <= 600.0,
                    "{label} clipped at {bounds:?}, import={importing}"
                );
            }
        }
    }

    #[test]
    fn table_lock_blocks_source_changes_but_allows_copy_and_scroll() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.snapshot_scene = None;
        let source = format!("#table([Keep])\n{}", "A scrollable line\n".repeat(100));
        app.document_mut()
            .replace_unprojected_untitled(source.clone());
        let frame = |app: &mut EditorApp, events| {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| app.show_editor(ui),
                )
                .drop_without_applying_deltas();
        };
        frame(&mut app, vec![]);
        app.begin_table_editor(editable_table_at(&source, 2).unwrap());
        let source_key = app.document().key();
        let editor_id = source_editor_id(&context);
        context.memory_mut(|memory| memory.request_focus(editor_id));
        frame(
            &mut app,
            vec![
                egui::Event::Text("WRONG".into()),
                egui::Event::Paste("WRONG".into()),
                key(egui::Key::Backspace, Modifiers::NONE),
            ],
        );
        app.select_all_editor(&context);
        app.cut_editor_selection(&context);
        app.undo_editor(&context, false);
        app.undo_editor(&context, true);
        app.toggle_comments(&context);
        app.request_format_document();
        app.find_bar.query = "Keep".into();
        app.find_bar.replacement = "WRONG".into();
        app.apply_find_actions(&context, false, false, true, true);
        assert!(!app.set_tex_mode(true, &context));
        assert_eq!(app.document().key(), source_key);
        assert_eq!(app.document().source(), &source);
        let output = context.run_ui(Default::default(), |ui| app.copy_editor_selection(ui.ctx()));
        assert!(output.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &source)
        ));
        output.drop_without_applying_deltas();
        let offset_id = viewport_scoped_id(&context, "source-hover-scroll-offset");
        let before = context
            .data(|data| data.get_temp::<Vec2>(offset_id))
            .unwrap();
        for _ in 0..5 {
            frame(
                &mut app,
                vec![
                    egui::Event::PointerMoved(Pos2::new(300.0, 250.0)),
                    egui::Event::MouseWheel {
                        phase: egui::TouchPhase::Move,
                        unit: egui::MouseWheelUnit::Point,
                        delta: Vec2::new(0.0, -160.0),
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
        }
        let after = context
            .data(|data| data.get_temp::<Vec2>(offset_id))
            .unwrap();
        assert!(
            after.y > before.y,
            "read-only source must scroll: {before:?} -> {after:?}"
        );
        app.table_editor = None;
        app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(0)));
        context.memory_mut(|memory| memory.request_focus(editor_id));
        frame(&mut app, vec![egui::Event::Text("Editable again ".into())]);
        assert!(app.document().source().starts_with("Editable again "));
    }

    #[test]
    fn new_table_is_a_unicode_safe_draft_and_applies_as_one_undo() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("Préface\n\nFin");
        app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(8)));
        app.begin_new_table(&context);
        assert_eq!(app.document().source(), "Préface\n\nFin");
        let mut dialog = app.table_editor.take().unwrap();
        dialog.table.cells[0][0] = "Été".into();
        let edit =
            prepare_table_source_edit(app.document().source(), app.document().key(), &dialog)
                .unwrap();
        assert!(edit.replacement.starts_with("#table("));
        app.document_mut()
            .edit(CCursorRange::one(CCursor::new(8)), |source| {
                source.replace_range(edit.byte_range, &edit.replacement)
            });
        assert!(app.document().source().contains("[Été]"));
        app.undo_editor(&context, false);
        assert_eq!(app.document().source(), "Préface\n\nFin");
        app.prepare_editor_source_data();
        app.editor_data.prepare_table_at_cursor(2);
        assert!(!app.native_command_enabled(AppCommand::EditTable));
        app.document_mut()
            .replace_unprojected_untitled("#table([A])");
        app.prepare_editor_source_data();
        app.editor_data.prepare_table_at_cursor(2);
        assert!(app.native_command_enabled(AppCommand::EditTable));
        app.begin_table_editor(editable_table_at(app.document().source(), 2).unwrap());
        assert!(!app.native_command_enabled(AppCommand::EditTable));
        assert!(!app.native_command_enabled(AppCommand::NewTable));
    }
    #[test]
    fn span_geometry_and_selection_include_covered_cells() {
        let origin = Pos2::new(10.0, 20.0);
        let a = cell_rect(origin, (0, 0), (2, 2), 160.0);
        let b = cell_rect(origin, (0, 2), (1, 1), 160.0);
        assert_eq!(a.size(), Vec2::new(314.0, 142.0));
        assert!(a.right() < b.left());
        assert!(selection_intersects_cell(
            TableSelection {
                anchor: (1, 1),
                focus: (1, 1)
            },
            (0, 0),
            (2, 2)
        ));
    }
}
