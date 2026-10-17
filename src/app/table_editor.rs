//! A nonmodal, draft-based table workbench. Source stays locked until Apply/Cancel.
use super::*;
use crate::editor_features::{CellPosition, TableAxis, TableSelection};

#[derive(Debug, Clone, Default)]
pub(super) struct TableEditorState {
    selection: TableSelection,
    focus_cell: Option<CellPosition>,
    edge: Option<(TableAxis, usize)>,
    edge_draft_for: Option<(TableAxis, usize)>,
    edge_draft: String,
    edge_error: Option<String>,
    rule_cache_key: Option<(u64, usize, usize)>,
    rule_cache: Vec<(TableAxis, usize, egui::Stroke)>,
    widths: Vec<f32>,
    width_drag: Option<(usize, f32, f32, f32)>,
    write_ratios: bool,
    importing: bool,
    markdown: String,
    source_requested: bool,
    format_selected: bool,
}

impl TableEditorState {
    pub(super) fn take_source_request(&mut self) -> bool {
        std::mem::take(&mut self.source_requested)
    }
}

pub(super) const CELL_HEIGHT: f32 = 36.0;
pub(super) const HEADER_HEIGHT: f32 = 24.0;
pub(super) const ROW_HEADER: f32 = 32.0;

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
            ("Blue", Some("1pt + blue")),
            ("Red", Some("1pt + red")),
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

#[cfg(test)]
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
}

fn cell_id(ui: &egui::Ui, cell: CellPosition) -> egui::Id {
    viewport_scoped_id(ui.ctx(), "table-cell").with(cell)
}

pub(super) fn show_table_editor_ui(
    ui: &mut egui::Ui,
    dialog: &mut TypstTableEditorDialog,
    available_width: f32,
    cells_height: f32,
    settings: &AppSettings,
) -> Option<TableEditorUiAction> {
    let has_locked = crate::table_sampling::view(ui).is_some_and(|view| !view.locked.is_empty());
    dialog
        .ui
        .selection
        .clamp(dialog.table.row_count(), dialog.table.columns);
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("Typst · {}", dialog.table.kind.callee()));
        ui.weak(format!(
            "{} × {}",
            dialog.table.row_count(),
            dialog.table.columns
        ));
        ui.separator();
        let add_row = ui.add_enabled(
            !has_locked && dialog.table.can_add_row(),
            egui::Button::new("+ Row"),
        );
        add_row.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, !has_locked, "Add row")
        });
        if add_row.clicked() {
            dialog.table.add_row();
        }
        let add_column = ui.add_enabled(
            !has_locked && dialog.table.can_add_column(),
            egui::Button::new("+ Column"),
        );
        add_column.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, !has_locked, "Add column")
        });
        if add_column.clicked() {
            dialog.table.add_column();
        }
        ui.menu_button("Remove", |ui| {
            if ui
                .add_enabled(!has_locked, egui::Button::new("Selected row"))
                .clicked()
            {
                dialog.table.remove_row(dialog.ui.selection.focus.0);
                ui.close();
            }
            if ui
                .add_enabled(
                    !has_locked && dialog.table.columns > 1,
                    egui::Button::new("Last column"),
                )
                .clicked()
            {
                dialog.table.remove_column();
                ui.close();
            }
        });
        let format = ui.menu_button("Format", |ui| {
            ui.set_width(360.0);
            format_controls(ui, dialog, has_locked);
        });
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("table.format", &format.response);
        let _ = format;
        ui.add_enabled_ui(!has_locked, |ui| {
            ui.toggle_value(&mut dialog.ui.importing, "Import Markdown")
        });
        if dialog.insertion_prefix.is_none() {
            let source = ui.button("Source");
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("table.source-mode", &source);
            if source.clicked() {
                dialog.ui.source_requested = true;
            }
        }
        if has_locked {
            ui.weak("Sampled cells locked");
        }
    });
    ui.separator();
    dialog
        .ui
        .selection
        .clamp(dialog.table.row_count(), dialog.table.columns);
    if has_locked {
        dialog.ui.importing = false;
    }
    if dialog.ui.importing {
        let height = table_grid_height(ui, cells_height);
        ui.label("Markdown");
        egui::ScrollArea::vertical()
            .id_salt("markdown-import-scroll")
            .max_height((height - 56.0).max(1.0))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut dialog.ui.markdown)
                        .code_editor()
                        .desired_rows(8)
                        .desired_width(f32::INFINITY)
                        .hint_text("| Name | Value |\n| :--- | ---: |\n| Alpha | 42 |"),
                );
            });
        if ui.button("Replace draft from Markdown").clicked() {
            match dialog.table.import_markdown(&dialog.ui.markdown) {
                Ok(()) => {
                    dialog.ui.selection = Default::default();
                    dialog.ui.edge = None;
                    dialog.ui.edge_draft_for = None;
                    dialog.ui.edge_error = None;
                    dialog.ui.widths.clear();
                    dialog.focus_first_cell = true;
                    dialog.ui.importing = false;
                    dialog.error = None;
                }
                Err(error) => dialog.error = Some(error),
            }
        }
        return table_footer(ui, dialog);
    }
    if let Some((axis, boundary)) = dialog.ui.edge {
        let limit = if axis == TableAxis::Horizontal {
            dialog.table.row_count()
        } else {
            dialog.table.columns
        };
        if boundary > limit {
            dialog.ui.edge = None;
            dialog.ui.edge_draft_for = None;
            dialog.ui.edge_error = None;
        } else {
            edge_controls(ui, dialog, axis, boundary);
        }
    }
    if dialog.focus_first_cell && dialog.table.row_count() > 0 {
        dialog.ui.focus_cell = Some((0, 0));
        dialog.focus_first_cell = false;
    }
    // The native wrapper prepares navigation before TextEdit consumes input.
    // Direct UI harnesses use the same cheap event gate; idle frames do no scan.
    prepare_grid_keyboard(ui, dialog);
    let owners = dialog.table.cell_owners().unwrap_or_default();
    let width =
        ((available_width - ROW_HEADER - 18.0) / dialog.table.columns.min(5) as f32).max(128.0);
    dialog.ui.widths.resize(dialog.table.columns, width);
    for width in &mut dialog.ui.widths {
        *width = width.clamp(72.0, 1200.0);
    }
    let height = table_grid_height(ui, cells_height);
    let scroll = egui::ScrollArea::both()
        .id_salt("table-editor-cells")
        .max_height(height)
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (_, area) = ui.allocate_space(Vec2::new(
                ROW_HEADER + dialog.ui.widths.iter().sum::<f32>(),
                HEADER_HEIGHT + dialog.table.row_count() as f32 * CELL_HEIGHT,
            ));
            let header = Rect::from_min_size(area.min, Vec2::new(area.width(), HEADER_HEIGHT));
            ui.painter()
                .rect_filled(header, 0.0, ui.visuals().panel_fill);
            header_width_handles(ui, dialog, area);
            let offsets = column_offsets(&dialog.ui.widths);
            for (c, &offset) in offsets.iter().enumerate().take(dialog.table.columns) {
                let rect = Rect::from_min_size(
                    area.min + Vec2::new(offset, 0.0),
                    Vec2::new(dialog.ui.widths[c], HEADER_HEIGHT),
                )
                .shrink(2.0);
                if ui.is_rect_visible(rect) {
                    let response = ui.put(
                        rect,
                        egui::Button::new(RichText::new(column_name(c)).small()).frame(false),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("Column {}", c + 1),
                        )
                    });
                    if response.clicked() {
                        dialog.ui.edge = None;
                        dialog.ui.edge_draft_for = None;
                        dialog.ui.edge_error = None;
                        dialog
                            .ui
                            .selection
                            .select_column(c, dialog.table.row_count());
                    }
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
                        .clicked()
                {
                    dialog.ui.edge = None;
                    dialog.ui.edge_draft_for = None;
                    dialog.ui.edge_error = None;
                    dialog.ui.selection.select_row(r, dialog.table.columns);
                }
                for (c, &owner) in row.iter().enumerate() {
                    if owner != (r, c) {
                        continue;
                    }
                    let rect =
                        variable_cell_rect(area.min, (r, c), dialog.table.span((r, c)), &offsets);
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
                    let fill = effective_style(&dialog.table, (r, c), "fill")
                        .as_deref()
                        .and_then(literal_color)
                        .unwrap_or(ui.visuals().extreme_bg_color);
                    ui.painter().rect_filled(rect, 0.0, fill);
                    let stroke = effective_style(&dialog.table, (r, c), "stroke")
                        .as_deref()
                        .and_then(literal_stroke)
                        .unwrap_or(ui.visuals().widgets.noninteractive.bg_stroke);
                    ui.painter()
                        .rect_stroke(rect, 0.0, stroke, StrokeKind::Middle);
                    if selected {
                        ui.painter().rect_stroke(
                            rect.shrink(1.0),
                            0.0,
                            ui.visuals().selection.stroke,
                            StrokeKind::Inside,
                        );
                    }
                    let mut inner = rect.shrink(4.0);
                    let alignment = effective_style(&dialog.table, (r, c), "align")
                        .as_deref()
                        .and_then(literal_alignment)
                        .unwrap_or((Align::Min, Align::Min));
                    let mut prepared = code_editor::highlight_job(
                        ui,
                        id,
                        &dialog.table.cells[r][c],
                        "typ",
                        settings,
                    );
                    for section in &mut prepared.sections {
                        section.format.color = readable_cell_color(section.format.color, fill);
                    }
                    prepared.wrap.max_width = inner.width();
                    let prepared = ui.fonts_mut(|fonts| fonts.layout_job(prepared));
                    let line_height = ui.fonts_mut(|fonts| fonts.row_height(&theme::editor_font()));
                    inner = aligned_content_rect(
                        inner,
                        prepared.size().y.max(line_height),
                        alignment.1,
                    );
                    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
                        if prepared.job.text == text.as_str()
                            && (width - prepared.job.wrap.max_width).abs() < 0.01
                        {
                            return prepared.clone();
                        }
                        let mut job =
                            code_editor::highlight_job(ui, id, text.as_str(), "typ", settings);
                        for section in &mut job.sections {
                            section.format.color = readable_cell_color(section.format.color, fill);
                        }
                        job.wrap.max_width = width;
                        ui.fonts_mut(|fonts| fonts.layout_job(job))
                    };
                    let response = ui
                        .scope_builder(
                            egui::UiBuilder::new()
                                .id_salt(("table-cell-editor", r, c))
                                .max_rect(inner),
                            |ui| {
                                ui.set_clip_rect(ui.clip_rect().intersect(inner));
                                let foreground =
                                    readable_cell_color(ui.visuals().text_color(), fill);
                                ui.visuals_mut().override_text_color = Some(foreground);
                                egui::ScrollArea::vertical()
                                    .id_salt(("table-cell-scroll", r, c))
                                    .min_scrolled_height(0.0)
                                    .max_height(inner.height())
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        if let Some(response) = table_samples::locked_cell(
                                            ui,
                                            (r, c),
                                            &dialog.table.cells[r][c],
                                        ) {
                                            return response;
                                        }
                                        ui.add(
                                            egui::TextEdit::multiline(
                                                &mut dialog.table.cells[r][c],
                                            )
                                            .id(id)
                                            .code_editor()
                                            .font(theme::editor_font())
                                            .horizontal_align(alignment.0)
                                            .layouter(&mut layouter)
                                            .frame(egui::Frame::NONE)
                                            .desired_width(ui.available_width())
                                            .desired_rows(1),
                                        )
                                    })
                                    .inner
                            },
                        )
                        .inner;
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(&format!("table.cell.{r}.{c}"), &response);
                    if response.clicked() || response.gained_focus() && !wants_focus {
                        dialog.ui.edge = None;
                        dialog.ui.edge_error = None;
                        dialog.ui.edge_draft_for = None;
                        dialog
                            .ui
                            .selection
                            .select((r, c), ui.input(|input| input.modifiers.shift));
                    }
                    if wants_focus {
                        if !crate::table_sampling::view(ui)
                            .is_some_and(|view| view.locked.contains_key(&(r, c)))
                        {
                            response.request_focus();
                            ui.scroll_to_rect(rect, Some(Align::Center));
                        }
                        dialog.ui.focus_cell = None;
                    }
                    if response.changed() {
                        dialog.error = None;
                    }
                }
            }
            table_edge_handles(ui, dialog, area, &offsets);
        });
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe_scroll(
        "table.grid",
        ui.ctx(),
        scroll.inner_rect,
        scroll.content_size.y,
        scroll.state.offset.y,
        false,
    );
    let _ = scroll;
    table_footer(ui, dialog)
}

pub(super) fn apply_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let accent = theme::palette(ui.ctx()).accent;
    ui.add_enabled(
        enabled,
        egui::Button::new(
            RichText::new("Apply").color(readable_cell_color(ui.visuals().text_color(), accent)),
        )
        .fill(accent),
    )
}

fn table_grid_height(ui: &egui::Ui, requested: f32) -> f32 {
    (ui.max_rect().bottom() - 48.0 - ui.cursor().top())
        .max(0.0)
        .min(requested.max(0.0))
}

fn table_footer(ui: &mut egui::Ui, dialog: &TypstTableEditorDialog) -> Option<TableEditorUiAction> {
    let bounds = Rect::from_min_max(
        Pos2::new(ui.max_rect().left(), ui.max_rect().bottom() - 44.0),
        ui.max_rect().max,
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("table-footer")
            .max_rect(bounds),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(bounds));
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut action = None;
                let apply =
                    apply_button(ui, !dialog.ui.importing && dialog.ui.edge_error.is_none());
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("table.apply", &apply);
                if apply.clicked() {
                    action = Some(TableEditorUiAction::Apply);
                }
                let cancel = ui.button("Cancel");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("table.cancel", &cancel);
                if cancel.clicked() {
                    action = Some(TableEditorUiAction::Cancel);
                }
                if let Some(error) = dialog.ui.edge_error.as_ref().or(dialog.error.as_ref()) {
                    ui.add(
                        egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color))
                            .truncate(),
                    );
                } else {
                    let (r, c) = dialog.ui.selection.focus;
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{}{}", column_name(c), r + 1)).weak(),
                        )
                        .truncate(),
                    );
                }
                action
            })
            .inner
        },
    )
    .inner
}

fn format_controls(ui: &mut egui::Ui, dialog: &mut TypstTableEditorDialog, has_locked: bool) {
    let owners = dialog.table.cell_owners().unwrap_or_default();
    let cells = selected_cells(dialog, &owners);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut dialog.ui.format_selected, false, "Whole table");
        ui.selectable_value(&mut dialog.ui.format_selected, true, "Selected cells");
    });
    ui.separator();
    ui.add_enabled_ui(!dialog.ui.format_selected || !cells.is_empty(), |ui| {
        for &(name, label, choices) in STYLE_CONTROLS {
            style_picker(
                ui,
                dialog,
                dialog.ui.format_selected,
                name,
                &cells,
                label,
                choices,
            );
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        let cell = owners
            .get(dialog.ui.selection.focus.0)
            .and_then(|row| row.get(dialog.ui.selection.focus.1))
            .copied()
            .unwrap_or((0, 0));
        let (mut rows, mut columns) = dialog.table.span(cell);
        ui.add_enabled_ui(!has_locked && cells.len() == 1, |ui| {
            ui.label("Span");
            let r = ui
                .add(
                    egui::DragValue::new(&mut rows)
                        .range(1..=dialog.table.row_count().saturating_sub(cell.0).max(1))
                        .speed(0.1),
                )
                .changed();
            ui.label("×");
            let c = ui
                .add(
                    egui::DragValue::new(&mut columns)
                        .range(1..=dialog.table.columns.saturating_sub(cell.1).max(1))
                        .speed(0.1),
                )
                .changed();
            if r || c {
                dialog.error = dialog
                    .table
                    .set_span(cell, rows, columns)
                    .err()
                    .map(|e| e.to_string());
            }
        });
    });
    let changed = ui
        .checkbox(&mut dialog.ui.write_ratios, "Write column ratios")
        .changed();
    if changed && !dialog.ui.write_ratios {
        dialog.table.reset_column_widths();
    }
}

pub(super) fn column_name(mut column: usize) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (column % 26) as u8);
        if column < 26 {
            break;
        }
        column = column / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap()
}

pub(super) fn readable_cell_color(
    color: egui::Color32,
    background: egui::Color32,
) -> egui::Color32 {
    let rgba = |c: egui::Color32| crate::sublime_theme::Rgba::from_rgba(c.r(), c.g(), c.b(), c.a());
    if rgba(color).contrast_ratio(rgba(background)) >= 4.5 {
        return color;
    }
    [
        egui::Color32::from_rgb(24, 27, 35),
        egui::Color32::from_rgb(250, 250, 252),
    ]
    .into_iter()
    .max_by(|a, b| {
        rgba(*a)
            .contrast_ratio(rgba(background))
            .total_cmp(&rgba(*b).contrast_ratio(rgba(background)))
    })
    .unwrap()
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

fn selected_cells(
    dialog: &TypstTableEditorDialog,
    owners: &[Vec<CellPosition>],
) -> Vec<CellPosition> {
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
    dialog: &mut TypstTableEditorDialog,
    selected: bool,
    name: &str,
    cells: &[CellPosition],
    label: &str,
    choices: &[(&str, Option<&str>)],
) {
    let target = if selected {
        cells.first().copied()
    } else {
        None
    };
    let value = dialog.table.style(target, name).map(str::to_owned);
    let mixed = selected
        && cells
            .iter()
            .any(|cell| dialog.table.style(Some(*cell), name) != value.as_deref());
    let current = if mixed {
        "Mixed"
    } else {
        choices
            .iter()
            .find(|(_, candidate)| *candidate == value.as_deref())
            .map_or("Custom", |(label, _)| label)
    };
    let mut change = None;
    ui.horizontal(|ui| {
        let label_response = ui.add_sized([68.0, 20.0], egui::Label::new(label));
        egui::ComboBox::from_id_salt(("table-style", selected, name))
            .selected_text(current)
            .width(86.0)
            .show_ui(ui, |ui| {
                for &(label, choice) in choices {
                    if ui
                        .selectable_label(!mixed && value.as_deref() == choice, label)
                        .clicked()
                    {
                        change = Some(choice.map(str::to_owned));
                    }
                }
            });
        let mut expression = if mixed {
            String::new()
        } else {
            value.clone().unwrap_or_default()
        };
        if ui
            .add(
                egui::TextEdit::singleline(&mut expression)
                    .code_editor()
                    .id_salt(("table-expression", selected, name))
                    .desired_width(ui.available_width())
                    .hint_text(if mixed {
                        "Mixed — enter an override"
                    } else {
                        "Default"
                    }),
            )
            .labelled_by(label_response.id)
            .changed()
        {
            change = Some((!expression.trim().is_empty()).then_some(expression));
        }
    });
    if let Some(value) = change {
        if selected {
            for &cell in cells {
                dialog.table.set_style(Some(cell), name, value.as_deref());
            }
        } else {
            dialog.table.set_style(None, name, value.as_deref());
        }
        dialog.error = None;
    }
}

fn column_offsets(widths: &[f32]) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(widths.len() + 1);
    offsets.push(ROW_HEADER);
    for width in widths {
        offsets.push(offsets.last().copied().unwrap() + width);
    }
    offsets
}

fn variable_cell_rect(
    origin: Pos2,
    (r, c): CellPosition,
    (rows, columns): (usize, usize),
    offsets: &[f32],
) -> Rect {
    Rect::from_min_size(
        origin + Vec2::new(offsets[c], HEADER_HEIGHT + r as f32 * CELL_HEIGHT),
        Vec2::new(offsets[c + columns] - offsets[c], rows as f32 * CELL_HEIGHT),
    )
}

fn header_width_handles(ui: &mut egui::Ui, dialog: &mut TypstTableEditorDialog, area: Rect) {
    let offsets = column_offsets(&dialog.ui.widths);
    let mut resized = false;
    for c in 0..dialog.table.columns.saturating_sub(1) {
        let rect = Rect::from_center_size(
            Pos2::new(
                area.left() + offsets[c + 1],
                area.top() + HEADER_HEIGHT * 0.5,
            ),
            Vec2::new(8.0, HEADER_HEIGHT),
        );
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let response = ui
            .interact(
                rect,
                ui.id().with(("table-column-width", c)),
                egui::Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                format!("Resize column {}", c + 1),
            )
        });
        ui.painter().line_segment(
            [
                Pos2::new(rect.center().x, rect.top() + 5.0),
                Pos2::new(rect.center().x, rect.bottom() - 5.0),
            ],
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        if response.drag_started()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            dialog.ui.width_drag =
                Some((c, pointer.x, dialog.ui.widths[c], dialog.ui.widths[c + 1]));
        }
        if response.dragged()
            && let (Some((index, start, left, right)), Some(pointer)) =
                (dialog.ui.width_drag, response.interact_pointer_pos())
            && index == c
        {
            let delta = (pointer.x - start).clamp(72.0 - left, right - 72.0);
            dialog.ui.widths[c] = left + delta;
            dialog.ui.widths[c + 1] = right - delta;
            resized = true;
        }
        if response.drag_stopped() {
            dialog.ui.width_drag = None;
        }
    }
    if resized && dialog.ui.write_ratios {
        match width_ratios(&dialog.ui.widths)
            .and_then(|values| dialog.table.set_column_widths(&values))
        {
            Ok(()) => dialog.error = None,
            Err(error) => dialog.error = Some(error),
        }
    }
}

fn width_ratios(widths: &[f32]) -> Result<Vec<String>, String> {
    let total: f32 = widths.iter().sum();
    if widths.is_empty()
        || !total.is_finite()
        || total <= 0.0
        || widths
            .iter()
            .any(|width| !width.is_finite() || *width <= 0.0)
    {
        return Err("Column widths must be positive".into());
    }
    Ok(widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            if index == widths.len() - 1 {
                "auto".into()
            } else {
                format!("{:.4}%", width / total * 100.0)
            }
        })
        .collect())
}

fn table_edge_handles(
    ui: &mut egui::Ui,
    dialog: &mut TypstTableEditorDialog,
    area: Rect,
    offsets: &[f32],
) {
    let left = area.left() + ROW_HEADER;
    let right = area.left() + offsets.last().copied().unwrap_or(ROW_HEADER);
    let top = area.top() + HEADER_HEIGHT;
    let bottom = top + dialog.table.row_count() as f32 * CELL_HEIGHT;
    let key = dialog.table.boundary_rules_key();
    if dialog.ui.rule_cache_key != Some(key) {
        dialog.ui.rule_cache = dialog
            .table
            .boundary_rules()
            .into_iter()
            .filter_map(|(axis, boundary, source)| {
                literal_stroke(&source).map(|stroke| (axis, boundary, stroke))
            })
            .collect();
        dialog.ui.rule_cache_key = Some(key);
    }
    let horizontal = (0..=dialog.table.row_count()).map(|boundary| {
        let y = top + boundary as f32 * CELL_HEIGHT;
        (
            TableAxis::Horizontal,
            boundary,
            Pos2::new(left, y),
            Pos2::new(right, y),
        )
    });
    let vertical = offsets
        .iter()
        .enumerate()
        .take(dialog.table.columns + 1)
        .map(|(boundary, &offset)| {
            let x = area.left() + offset;
            (
                TableAxis::Vertical,
                boundary,
                Pos2::new(x, top),
                Pos2::new(x, bottom),
            )
        });
    for (axis, boundary, from, to) in horizontal.chain(vertical) {
        let hit = Rect::from_two_pos(from, to).expand(3.0);
        if !ui.is_rect_visible(hit) {
            continue;
        }
        if let Some((_, _, stroke)) =
            dialog
                .ui
                .rule_cache
                .iter()
                .rev()
                .find(|(candidate_axis, candidate_boundary, _)| {
                    *candidate_axis == axis && *candidate_boundary == boundary
                })
        {
            ui.painter().line_segment([from, to], *stroke);
        }
        let response = ui
            .interact(
                hit,
                ui.id()
                    .with(("table-edge", axis == TableAxis::Horizontal, boundary)),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                format!(
                    "{} edge {}",
                    if axis == TableAxis::Horizontal {
                        "Row"
                    } else {
                        "Column"
                    },
                    boundary + 1
                ),
            )
        });
        if response.clicked() {
            dialog.ui.edge = Some((axis, boundary));
            if let Some(focused) = ui.memory(|memory| memory.focused()) {
                ui.memory_mut(|memory| memory.surrender_focus(focused));
            }
        }
        if response.hovered() || dialog.ui.edge == Some((axis, boundary)) {
            ui.painter()
                .line_segment([from, to], ui.visuals().selection.stroke);
        }
    }
}

fn edge_controls(
    ui: &mut egui::Ui,
    dialog: &mut TypstTableEditorDialog,
    axis: TableAxis,
    boundary: usize,
) {
    if dialog.ui.edge_draft_for != Some((axis, boundary)) {
        dialog.ui.edge_draft = dialog
            .table
            .boundary_rule(axis, boundary)
            .unwrap_or_default();
        dialog.ui.edge_draft_for = Some((axis, boundary));
        dialog.ui.edge_error = None;
    }
    ui.horizontal(|ui| {
        ui.strong(format!(
            "{} edge {}",
            if axis == TableAxis::Horizontal {
                "Row"
            } else {
                "Column"
            },
            boundary + 1
        ));
        let changed = ui
            .add(
                egui::TextEdit::singleline(&mut dialog.ui.edge_draft)
                    .code_editor()
                    .id_salt("table-edge-stroke")
                    .hint_text("Default stroke")
                    .desired_width(160.0),
            )
            .changed();
        let none = ui.small_button("None").clicked();
        let reset = ui.small_button("Default").clicked();
        let done = ui.small_button("Done").clicked();
        if none {
            dialog.ui.edge_draft = "none".into();
        }
        if reset {
            dialog.ui.edge_draft.clear();
        }
        if changed || none || reset || done {
            let value =
                (!dialog.ui.edge_draft.trim().is_empty()).then_some(dialog.ui.edge_draft.as_str());
            dialog.ui.edge_error = dialog.table.set_boundary_rule(axis, boundary, value).err();
            if dialog.ui.edge_error.is_none() {
                dialog.error = None;
                if done {
                    dialog.ui.edge = None;
                    dialog.ui.edge_draft_for = None;
                }
            }
        }
    });
}

/// Only literals are evaluated by the draft view. Functions, dictionaries,
/// gradients and other expressions remain intact for the Typst compiler.
fn effective_style(table: &EditableTable, cell: CellPosition, name: &str) -> Option<String> {
    table
        .style(Some(cell), name)
        .filter(|value| *value != "auto")
        .or_else(|| table.style(None, name))
        .map(str::to_owned)
}

fn literal_alignment(source: &str) -> Option<(Align, Align)> {
    let mut result = (Align::Min, Align::Min);
    for term in source.split('+').map(str::trim) {
        match term {
            "left" | "start" => result.0 = Align::Min,
            "center" => result.0 = Align::Center,
            "right" | "end" => result.0 = Align::Max,
            "top" => result.1 = Align::Min,
            "horizon" => result.1 = Align::Center,
            "bottom" => result.1 = Align::Max,
            _ => return None,
        }
    }
    Some(result)
}

fn aligned_content_rect(inner: Rect, content_height: f32, align: Align) -> Rect {
    let height = content_height.min(inner.height()).max(0.0);
    let top = match align {
        Align::Min => inner.top(),
        Align::Center => inner.center().y - height * 0.5,
        Align::Max => inner.bottom() - height,
    };
    Rect::from_min_size(
        Pos2::new(inner.left(), top),
        Vec2::new(inner.width(), height),
    )
}

fn literal_length(source: &str) -> Option<f32> {
    let source = source.trim();
    let value = if let Some(value) = source.strip_suffix("pt") {
        value.trim().parse::<f32>().ok()?
    } else if let Some(value) = source.strip_suffix("cm") {
        value.trim().parse::<f32>().ok()? * 72.0 / 2.54
    } else {
        let value = source.strip_suffix("mm")?;
        value.trim().parse::<f32>().ok()? * 72.0 / 25.4
    };
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn literal_color(source: &str) -> Option<egui::Color32> {
    let source = source
        .trim()
        .strip_prefix("color.")
        .unwrap_or(source.trim());
    let color = match source {
        "none" => return Some(egui::Color32::TRANSPARENT),
        "black" => 0x000000,
        "gray" => 0xaaaaaa,
        "silver" => 0xdddddd,
        "white" => 0xffffff,
        "navy" => 0x001f3f,
        "blue" => 0x0074d9,
        "aqua" => 0x7fdbff,
        "teal" => 0x39cccc,
        "eastern" => 0x239dad,
        "purple" => 0xb10dc9,
        "fuchsia" => 0xf012be,
        "maroon" => 0x85144b,
        "red" => 0xff4136,
        "orange" => 0xff851b,
        "yellow" => 0xffdc00,
        "olive" => 0x3d9970,
        "green" => 0x2ecc40,
        "lime" => 0x01ff70,
        _ => {
            let argument = source
                .strip_prefix("rgb(")?
                .strip_suffix(')')?
                .trim()
                .strip_prefix('"')?
                .strip_suffix('"')?;
            let hex = argument.strip_prefix('#').unwrap_or(argument);
            let expanded = if matches!(hex.len(), 3 | 4) {
                hex.chars().flat_map(|ch| [ch, ch]).collect::<String>()
            } else {
                hex.to_owned()
            };
            if !matches!(expanded.len(), 6 | 8)
                || !expanded.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            let value = u32::from_str_radix(&expanded, 16).ok()?;
            return Some(if expanded.len() == 8 {
                egui::Color32::from_rgba_unmultiplied(
                    (value >> 24) as u8,
                    (value >> 16) as u8,
                    (value >> 8) as u8,
                    value as u8,
                )
            } else {
                egui::Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
            });
        }
    };
    Some(egui::Color32::from_rgb(
        (color >> 16) as u8,
        (color >> 8) as u8,
        color as u8,
    ))
}

fn literal_stroke(source: &str) -> Option<egui::Stroke> {
    if source.trim() == "none" {
        return Some(egui::Stroke::NONE);
    }
    let mut width = None;
    let mut color = None;
    for term in source.split('+').map(str::trim) {
        if let Some(value) = literal_length(term) {
            if width.replace(value).is_some() {
                return None;
            }
        } else {
            let value = literal_color(term)?;
            if color.replace(value).is_some() {
                return None;
            }
        }
    }
    Some(egui::Stroke::new(
        width.unwrap_or(1.0).clamp(0.0, 16.0),
        color.unwrap_or(egui::Color32::BLACK),
    ))
}

pub(super) fn prepare_grid_keyboard(ui: &mut egui::Ui, dialog: &mut TypstTableEditorDialog) {
    // Cell navigation owns its input before the generic field normalizer can
    // turn Shift+Cmd+A into Select All or defer Ctrl+Space as completion.
    // Avoid rebuilding the owner map during idle paints and ordinary typing.
    let has_grid_key = ui.input(|input| {
        input.events.iter().any(|event| {
            let egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                ..
            } = event
            else {
                return false;
            };
            match key {
                egui::Key::A => modifiers.command && modifiers.shift,
                egui::Key::Space => modifiers.ctrl || modifiers.shift,
                egui::Key::Tab => true,
                egui::Key::ArrowUp
                | egui::Key::ArrowDown
                | egui::Key::ArrowLeft
                | egui::Key::ArrowRight => modifiers.alt,
                _ => false,
            }
        })
    });
    if has_grid_key && let Ok(owners) = dialog.table.cell_owners() {
        keyboard_selection(ui, dialog, &owners);
    }
}

fn keyboard_selection(
    ui: &mut egui::Ui,
    dialog: &mut TypstTableEditorDialog,
    owners: &[Vec<CellPosition>],
) {
    let samples = crate::table_sampling::view(ui);
    let anchors: Vec<_> = owners
        .iter()
        .enumerate()
        .flat_map(|(r, row)| {
            row.iter()
                .enumerate()
                .filter_map(move |(c, &owner)| (owner == (r, c)).then_some(owner))
        })
        .filter(|position| {
            !samples
                .as_ref()
                .is_some_and(|view| view.locked.contains_key(position))
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
        if !samples
            .as_ref()
            .is_some_and(|view| view.locked.contains_key(&cell))
        {
            dialog.ui.focus_cell = Some(cell);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(source: &str) -> TypstTableEditorDialog {
        let table = editable_table_at(source, 2).unwrap();
        TypstTableEditorDialog {
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
    fn long_editable_table_cell_scroll_reaches_last_line_and_keeps_footer_fixed() {
        fn geometry(shape: &egui::Shape, text: &str) -> Option<Rect> {
            match shape {
                egui::Shape::Text(value) if value.galley.job.text == text => {
                    Some(Rect::from_min_size(value.pos, value.galley.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| geometry(shape, text)),
                _ => None,
            }
        }
        fn frame(
            context: &egui::Context,
            dialog: &mut TableEditorDialog,
            text: &str,
            index: usize,
            events: Vec<egui::Event>,
        ) -> Option<(Rect, Rect, Rect)> {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(620.0, 420.0))),
                    time: Some(index as f64 / 60.0),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let _ =
                        table_workbench::show(ui, dialog, 620.0, 420.0, &AppSettings::default());
                },
            );
            let cell = output
                .shapes
                .iter()
                .find_map(|shape| geometry(&shape.shape, text).map(|rect| (rect, shape.clip_rect)));
            let footer = output
                .shapes
                .iter()
                .find_map(|shape| geometry(&shape.shape, "Apply"));
            output.drop_without_applying_deltas();
            cell.zip(footer)
                .map(|((rect, clip), footer)| (rect, clip, footer))
        }
        let context = egui::Context::default();
        theme::configure_test_fonts(&context);
        let text = (0..40)
            .map(|line| format!("Editable line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut draft = dialog(&format!("#table(columns: 1, [{text}])"));
        draft.focus_first_cell = false;
        let original = draft.table.render_source_draft().unwrap();
        let mut draft = TableEditorDialog::Typst(Box::new(draft));
        let _ = frame(&context, &mut draft, &text, 0, Vec::new());
        let (before, clip, footer) = frame(&context, &mut draft, &text, 1, Vec::new())
            .expect("the editable value and footer must render");
        assert!(before.bottom() > clip.bottom() + 100.0);
        assert!(footer.bottom() <= 420.0);
        let pointer = before.intersect(clip).center();
        let mut after = before;
        let mut after_clip = clip;
        for index in 2..22 {
            let mut events = vec![egui::Event::PointerMoved(pointer)];
            if index == 2 {
                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -8_000.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                });
            }
            let current = frame(&context, &mut draft, &text, index, events)
                .expect("the editable value must remain present after scrolling");
            (after, after_clip) = (current.0, current.1);
            assert_eq!(current.2, footer, "scrolling a cell must not move Apply");
        }
        assert!(
            after.bottom() <= after_clip.bottom() + 1.0 && after.bottom() >= after_clip.top(),
            "the last editable line must enter the cell clip: {after:?}, {after_clip:?}"
        );
        let TableEditorDialog::Typst(draft) = draft else {
            unreachable!()
        };
        assert_eq!(draft.table.render_source_draft().unwrap(), original);
    }

    #[test]
    fn table_samples_lock_values_skip_keyboard_focus_and_preserve_expressions() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT as _, Queryable as _, by},
        };
        let original = dialog("#table(columns: 3, [A], [#price], [C])");
        let mut ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(940.0, 800.0))
            .build_ui_state(
                move |ui, dialog: &mut TypstTableEditorDialog| {
                    if !ready {
                        theme::configure_test_fonts(ui.ctx());
                        ready = true;
                        return;
                    }
                    crate::table_sampling::publish(
                        ui.ctx(),
                        ui.ctx().viewport_id(),
                        Arc::new(crate::table_sampling::View {
                            locked: std::collections::BTreeMap::from([((0, 1), "#price".into())]),
                            grid: Some(crate::table_sampling::Grid {
                                columns: 3,
                                rows: 1,
                                instances: 1,
                                cells: vec![crate::table_sampling::Cell {
                                    position: (0, 1),
                                    span: (1, 1),
                                    value: "60".into(),
                                }],
                            }),
                            ..Default::default()
                        }),
                    );
                    show_table_editor_ui(ui, dialog, 920.0, 780.0, &AppSettings::default());
                },
                original,
            );
        harness.run();
        assert_eq!(
            harness
                .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
                .count(),
            2
        );
        assert!(
            harness
                .get_by_label("Add row")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            harness
                .get_by_label("Add column")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("60").click();
        harness.event(egui::Event::Text("overwrite".into()));
        harness.run();
        assert_eq!(harness.state().table.cells[0], ["A", "#price", "C"]);
        harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("A"),
            )
            .click();
        harness.key_press(egui::Key::Tab);
        harness.run();
        assert_eq!(harness.state().ui.selection.focus, (0, 2));
        harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("C"),
            )
            .type_text("literal");
        harness.run();
        let rendered = harness.state().table.render_source_draft().unwrap();
        assert!(rendered.contains("[#price]"));
        assert!(rendered.contains("literal"));
    }

    #[test]
    fn variable_column_geometry_keeps_spans_and_edges_aligned() {
        let offsets = column_offsets(&[100.0, 200.0, 300.0]);
        assert_eq!(offsets, [32.0, 132.0, 332.0, 632.0]);
        let origin = Pos2::new(10.0, 20.0);
        let span = variable_cell_rect(origin, (0, 0), (2, 2), &offsets);
        let next = variable_cell_rect(origin, (0, 2), (1, 1), &offsets);
        assert_eq!(span.size(), Vec2::new(300.0, 72.0));
        assert_eq!(next.left(), span.right());
        assert_eq!(span.left(), origin.x + ROW_HEADER);
    }

    #[test]
    fn literal_vertical_alignment_places_short_content_without_changing_cell_bounds() {
        let inner = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(150.0, 100.0));
        assert_eq!(aligned_content_rect(inner, 30.0, Align::Min).top(), 20.0);
        assert_eq!(aligned_content_rect(inner, 30.0, Align::Center).top(), 55.0);
        assert_eq!(
            aligned_content_rect(inner, 30.0, Align::Max).bottom(),
            120.0
        );
        assert_eq!(aligned_content_rect(inner, 500.0, Align::Max), inner);
        assert_eq!(
            literal_alignment("right + bottom"),
            Some((Align::Max, Align::Max))
        );
        assert_eq!(literal_alignment("(x, y) => right"), None);
    }

    #[test]
    fn header_ratios_are_bounded_and_leave_the_final_column_auto() {
        assert_eq!(
            width_ratios(&[100.0, 200.0, 100.0]).unwrap(),
            ["25.0000%", "50.0000%", "auto"]
        );
        assert_eq!(width_ratios(&[100.0]).unwrap(), ["auto"]);
        for widths in [
            vec![],
            vec![0.0],
            vec![f32::NAN],
            vec![-1.0],
            vec![f32::INFINITY],
        ] {
            assert!(width_ratios(&widths).is_err());
        }
        let state = TableEditorState::default();
        assert!(!state.write_ratios);
    }

    #[test]
    fn literal_styles_change_colors_and_widths_without_evaluating_source() {
        assert_eq!(
            literal_color("red"),
            Some(egui::Color32::from_rgb(255, 65, 54))
        );
        assert_eq!(
            literal_color("rgb(\"#dbeafe\")"),
            Some(egui::Color32::from_rgb(219, 234, 254))
        );
        assert_eq!(
            literal_color("rgb(\"#abc\")"),
            Some(egui::Color32::from_rgb(170, 187, 204))
        );
        assert_eq!(
            literal_stroke("0.5pt + blue"),
            Some(egui::Stroke::new(0.5, egui::Color32::from_rgb(0, 116, 217)))
        );
        assert_eq!(literal_stroke("none"), Some(egui::Stroke::NONE));
        for source in ["theme-color", "rgb(\"#invalid\")", "(x, y) => red"] {
            assert!(literal_color(source).is_none());
        }
        for source in ["custom-stroke", "1pt + 2pt", "red + blue", "-1pt"] {
            assert!(literal_stroke(source).is_none());
        }
    }

    #[test]
    fn invalid_intermediate_edge_expression_survives_frames_and_blocks_apply() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut draft = dialog("#table(columns: 2, [A], [B])");
        draft.ui.edge = Some((TableAxis::Horizontal, 0));
        draft.ui.edge_draft_for = draft.ui.edge;
        draft.ui.edge_draft = "rgb(\"".into();
        draft.ui.edge_error = Some("Finish the stroke expression".into());
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(940.0, 740.0))
            .build_ui_state(
                move |ui, dialog: &mut TypstTableEditorDialog| {
                    if !configured {
                        theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        return;
                    }
                    show_table_editor_ui(ui, dialog, 920.0, 720.0, &AppSettings::default());
                },
                draft,
            );
        harness.run();
        assert_eq!(harness.state().ui.edge_draft, "rgb(\"");
        assert!(harness.state().ui.edge_error.is_some());
        harness.get_by_label("Done").click();
        harness.run();
        assert_eq!(harness.state().ui.edge_draft, "rgb(\"");
        assert!(harness.state().ui.edge_error.is_some());
        assert!(
            harness
                .state()
                .table
                .boundary_rule(TableAxis::Horizontal, 0)
                .is_none()
        );
    }

    #[test]
    fn formatting_is_disclosed_without_moving_the_grid_or_footer() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT as _, Queryable as _},
        };
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(620.0, 560.0))
            .build_ui_state(
                move |ui, dialog| {
                    if !configured {
                        theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        return;
                    }
                    show_table_editor_ui(ui, dialog, 600.0, 540.0, &AppSettings::default());
                },
                dialog("#table(columns: 2, [A], [B])"),
            );
        harness.run();
        assert_eq!(harness.query_all_by_label("Border").count(), 0);
        let before = harness
            .get_by_label("Apply")
            .accesskit_node()
            .bounding_box()
            .unwrap();
        harness.get_by_label("Format").click();
        harness.run();
        assert_eq!(harness.query_all_by_label("Border").count(), 1);
        harness.get_by_label("Selected cells").click();
        harness.run();
        assert!(harness.state().ui.format_selected);
        assert_eq!(
            harness
                .get_by_label("Apply")
                .accesskit_node()
                .bounding_box()
                .unwrap(),
            before
        );
    }

    #[test]
    fn filled_cells_keep_readable_text_in_both_appearances() {
        use crate::sublime_theme::Rgba;
        for background in [
            egui::Color32::from_rgb(219, 234, 254),
            egui::Color32::from_rgb(30, 30, 46),
        ] {
            for foreground in [
                egui::Color32::from_rgb(205, 214, 244),
                egui::Color32::from_rgb(76, 79, 105),
            ] {
                let actual = readable_cell_color(foreground, background);
                assert!(
                    Rgba::rgb(actual.r(), actual.g(), actual.b()).contrast_ratio(Rgba::rgb(
                        background.r(),
                        background.g(),
                        background.b()
                    )) >= 4.5
                );
            }
        }
        assert_eq!([0, 25, 26, 127].map(column_name), ["A", "Z", "AA", "DX"]);
    }

    #[test]
    fn table_keyboard_moves_between_anchors_and_extends_selection() {
        use egui_kittest::Harness;
        let dialog = dialog("#table(columns: 3, table.cell(colspan: 2)[A], [B], [C], [D], [E])");
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(940.0, 700.0))
            .build_ui_state(
                move |ui, dialog: &mut TypstTableEditorDialog| {
                    if !configured {
                        theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        return;
                    }
                    show_table_editor_ui(ui, dialog, 920.0, 680.0, &AppSettings::default());
                },
                dialog,
            );
        harness.run();
        harness.key_press(egui::Key::Tab);
        harness.run();
        assert_eq!(harness.state().ui.selection.focus, (0, 2));
        harness.key_press_modifiers(Modifiers::ALT | Modifiers::SHIFT, egui::Key::ArrowDown);
        harness.run();
        assert_eq!(harness.state().ui.selection.anchor, (0, 2));
        assert_eq!(harness.state().ui.selection.focus, (1, 2));
        harness.key_press_modifiers(Modifiers::SHIFT, egui::Key::Space);
        harness.run();
        assert_eq!(harness.state().ui.selection.anchor, (1, 0));
        assert_eq!(harness.state().ui.selection.focus, (1, 2));
        harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::A);
        harness.run();
        assert_eq!(harness.state().ui.selection.anchor, (0, 0));
        assert_eq!(
            selected_cells(
                harness.state(),
                &harness.state().table.cell_owners().unwrap()
            )
            .len(),
            5
        );
    }

    #[test]
    fn native_table_wrapper_preserves_grid_keyboard_selection_and_navigation() {
        use egui_kittest::Harness;
        let cases = [
            (
                egui::Key::A,
                Modifiers::COMMAND | Modifiers::SHIFT,
                (0, 0),
                (1, 1),
            ),
            (egui::Key::Space, Modifiers::CTRL, (0, 0), (1, 0)),
            (egui::Key::Space, Modifiers::SHIFT, (0, 0), (0, 1)),
            (egui::Key::Tab, Modifiers::NONE, (0, 1), (0, 1)),
            (egui::Key::Tab, Modifiers::SHIFT, (1, 1), (1, 1)),
            (egui::Key::ArrowDown, Modifiers::ALT, (1, 0), (1, 0)),
            (
                egui::Key::ArrowRight,
                Modifiers::ALT | Modifiers::SHIFT,
                (0, 0),
                (0, 1),
            ),
        ];
        for (key, modifiers, anchor, focus) in cases {
            let original = dialog("#table(columns: 2, [A], [B], [C], [D])");
            let original_cells = original.table.cells.clone();
            let mut configured = false;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(940.0, 740.0))
                .build_ui_state(
                    move |ui, dialog: &mut table_workbench::TableEditorDialog| {
                        if !configured {
                            theme::configure_test_fonts(ui.ctx());
                            configured = true;
                            return;
                        }
                        // Exercise the native wrapper's preparation, rather
                        // than bypassing it by rendering the grid directly.
                        let settings = AppSettings::default();
                        let _physical = table_workbench::prepare_child_keyboard(
                            ui,
                            dialog,
                            &settings.effective_shortcuts(),
                            false,
                        );
                        table_workbench::show(ui, dialog, 920.0, 720.0, &settings);
                    },
                    table_workbench::TableEditorDialog::Typst(Box::new(original)),
                );
            harness.run();
            harness.key_press_modifiers(modifiers, key);
            harness.run();
            let table_workbench::TableEditorDialog::Typst(dialog) = harness.state() else {
                unreachable!()
            };
            assert_eq!(dialog.ui.selection.anchor, anchor, "{modifiers:?} {key:?}");
            assert_eq!(dialog.ui.selection.focus, focus, "{modifiers:?} {key:?}");
            assert_eq!(dialog.table.cells, original_cells);
        }
    }

    #[test]
    fn table_minimum_size_keeps_apply_and_cancel_visible() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT as _, Queryable as _},
        };
        for (importing, edge_error) in [(false, false), (true, false), (false, true)] {
            let mut dialog = dialog("#table(columns: 2, [A], [B], [C], [D])");
            dialog.table.cells[0][0] = "Long cell\n\n".repeat(80);
            dialog.ui.importing = importing;
            if edge_error {
                dialog.ui.edge = Some((TableAxis::Horizontal, 0));
                dialog.ui.edge_draft_for = dialog.ui.edge;
                dialog.ui.edge_draft = "rgb(\"".into();
                dialog.ui.edge_error =
                    Some("Finish the stroke expression before applying this table".into());
            }
            let mut configured = false;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(600.0, 520.0))
                .build_ui_state(
                    move |ui, dialog| {
                        if !configured {
                            theme::configure_test_fonts(ui.ctx());
                            configured = true;
                            return;
                        }
                        show_table_editor_ui(ui, dialog, 580.0, 500.0, &AppSettings::default());
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
                    "{label} clipped at {bounds:?}, import={importing}, edge_error={edge_error}"
                );
            }
            if edge_error {
                assert!(harness.get_by_label("Apply").accesskit_node().is_disabled());
                assert!(
                    !harness
                        .get_by_label("Cancel")
                        .accesskit_node()
                        .is_disabled()
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
        let TableEditorDialog::Typst(typst) = &mut dialog else {
            panic!("Expected a Typst table draft");
        };
        typst.table.cells[0][0] = "Été".into();
        let edit = table_workbench::prepare_source_edit(
            app.document().source(),
            app.document().key(),
            &dialog,
        )
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
        app.editor_data
            .prepare_table_at_cursor(2, DocumentKind::Typst);
        assert!(!app.native_command_enabled(AppCommand::EditTable));
        app.document_mut()
            .replace_unprojected_untitled("#table([A])");
        app.prepare_editor_source_data();
        app.editor_data
            .prepare_table_at_cursor(2, DocumentKind::Typst);
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
        assert_eq!(a.size(), Vec2::new(320.0, 72.0));
        assert_eq!(a.right(), b.left());
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
