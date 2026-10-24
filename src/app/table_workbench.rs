//! One draft lifetime and Apply/Cancel pipeline for Typst and TeX tables.
use super::*;
use crate::editor_features::{MathTable, MathTableKind, TexTable};

#[derive(Debug, Clone)]
pub(super) enum TableEditorDialog {
    Typst(Box<TypstTableEditorDialog>),
    Tex(Box<TexDialog>),
    Math(Box<MathDialog>),
}
#[derive(Debug, Clone)]
pub(super) struct TexDialog {
    pub(super) table: TexTable,
    original: String,
    column_spec: String,
    key: DocumentKey,
    error: Option<String>,
    validation: Option<String>,
}
#[derive(Debug, Clone)]
pub(super) struct MathDialog {
    pub(super) table: MathTable,
    original: String,
    key: DocumentKey,
    error: Option<String>,
    validation: Option<String>,
}
impl TableEditorDialog {
    pub(super) fn is_math(&self) -> bool {
        match self {
            Self::Math(_) => true,
            Self::Tex(dialog) => dialog.table.is_math(),
            Self::Typst(_) => false,
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn desktop_inspection(&self) -> serde_json::Value {
        let (rows, columns, valid, cells) = match self {
            Self::Typst(dialog) => (
                dialog.table.row_count(),
                dialog.table.columns,
                true,
                &dialog.table.cells,
            ),
            Self::Math(dialog) => (
                dialog.table.row_count(),
                dialog.table.columns,
                dialog.validation.is_none(),
                &dialog.table.cells,
            ),
            Self::Tex(dialog) => {
                let cells = dialog
                    .table
                    .cells
                    .as_ref()
                    .expect("admitted TeX table has a grid");
                (
                    cells.len(),
                    dialog.table.columns().unwrap_or_default(),
                    dialog.validation.is_none(),
                    cells,
                )
            }
        };
        let serialized = serde_json::to_string(cells).unwrap_or_default();
        serde_json::json!({
            "fingerprint": crate::desktop_test::fingerprint(&serialized),
            "rows": rows,
            "columns": columns,
            "valid": valid,
            "math": self.is_math(),
            "kind": match self {
                Self::Typst(dialog) => dialog.table.kind.callee(),
                Self::Tex(dialog) => dialog.table.environment.name(),
                Self::Math(dialog) => match dialog.table.kind {
                    MathTableKind::Matrix => "mat",
                    MathTableKind::Vector => "vec",
                    MathTableKind::Cases => "cases",
                },
            },
        })
    }

    pub(super) fn set_error(&mut self, error: String) {
        match self {
            Self::Typst(dialog) => dialog.error = Some(error),
            Self::Tex(dialog) => dialog.error = Some(error),
            Self::Math(dialog) => dialog.error = Some(error),
        }
    }
}
impl EditorApp {
    pub(super) fn begin_new_tex_table(&mut self, context: &egui::Context) {
        if self.overleaf_source_locked()
            || self.table_editor.is_some()
            || self.tabs.is_empty()
            || self.document().kind() != DocumentKind::Tex
        {
            return;
        }
        let cursor = self.editor_snapshot(context).cursor.primary.index.0;
        let dialog =
            match new_tex_table_draft(self.document().source(), cursor, self.document().key()) {
                Ok(dialog) => dialog,
                Err(message) => {
                    self.notice = Some(Notice {
                        message,
                        kind: NoticeKind::Info,
                    });
                    return;
                }
            };
        self.table_samples.reset();
        self.table_edit_events.clear();
        self.table_editor = Some(TableEditorDialog::Tex(Box::new(dialog)));
        self.editor_completion = None;
        self.format_request_key = None;
        self.manual_format_revision = None;
    }

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
    pub(super) fn begin_table_target(&mut self, target: TableTarget) {
        if self.overleaf_source_locked() {
            return;
        }
        if self.table_editor.is_some() {
            return;
        }
        self.table_samples.reset();
        self.table_edit_events.clear();
        let key = self.document().key();
        self.table_editor = match target {
            TableTarget::Typst(table) => {
                self.begin_table_editor(table);
                return;
            }
            TableTarget::TypstMath(table) => Some(TableEditorDialog::Math(Box::new(MathDialog {
                original: table.source.clone(),
                validation: table.source_edit().err().map(|error| error.to_string()),
                table,
                key,
                error: None,
            }))),
            TableTarget::Tex(table) => Some(TableEditorDialog::Tex(Box::new(TexDialog {
                original: table.source.clone(),
                column_spec: table.column_spec.clone(),
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

const EMPTY_TEX_TABLE: &str = r"\begin{tabular}{ll}
  & \\
  & \\
\end{tabular}";

fn new_tex_table_draft(source: &str, cursor: usize, key: DocumentKey) -> Result<TexDialog, String> {
    let byte =
        char_index_to_byte(source, cursor).ok_or("The insertion position is no longer valid")?;
    if !tex_table_text_context(source, byte) {
        return Err("Place the cursor in document text, outside comments, literal text, math, and command arguments, to insert a table".into());
    }
    let mut table = crate::editor_features::tex_table_at(EMPTY_TEX_TABLE, 1)
        .expect("the empty TeX table is a valid grid");
    table.source_range = cursor..cursor;
    Ok(TexDialog {
        original: String::new(),
        column_spec: table.column_spec.clone(),
        validation: table.grid_source_edit().err(),
        table,
        key,
        error: None,
    })
}

/// Conservative block admission: never split a command, group, comment or
/// literal payload. The shared context reader owns TeX math/environment rules.
fn tex_table_text_context(source: &str, byte: usize) -> bool {
    let Some(prefix) = source.get(..byte) else {
        return false;
    };
    let bytes = prefix.as_bytes();
    let mut context_source = std::borrow::Cow::Borrowed(prefix);
    let mut depth = 0usize;
    let mut brackets = 0usize;
    let mut pending_arguments = None;
    let mut pending_control = false;
    let mut at = 0;
    while at < prefix.len() {
        match bytes[at] {
            b'%' => {
                let Some(end) = prefix[at..].find('\n') else {
                    return false;
                };
                at += end + 1;
            }
            b'[' if depth == 0 => {
                brackets += 1;
                at += 1;
            }
            b']' if depth == 0 => {
                brackets = brackets.saturating_sub(1);
                at += 1;
            }
            b'{' => {
                if depth == 0 && brackets == 0 {
                    let unknown_control = pending_arguments.is_none() && pending_control;
                    pending_arguments = pending_arguments
                        .and_then(|count: usize| count.checked_sub(1))
                        .filter(|&count| count > 0);
                    pending_control = unknown_control || pending_arguments.is_some();
                }
                depth += 1;
                at += 1;
            }
            b'}' => {
                let Some(next) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next;
                at += 1;
            }
            b'\\' => {
                if let Some(literal) = crate::editor_math_guides::tex_inline_literal(prefix, at) {
                    if !literal.closed {
                        return false;
                    }
                    at = literal.end;
                    if depth == 0 && brackets == 0 {
                        pending_arguments = None;
                        pending_control = false;
                    }
                    continue;
                }
                let start = at;
                at += 1;
                let name_start = at;
                while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
                    at += 1;
                }
                if at == name_start {
                    let Some(ch) = prefix[at..].chars().next() else {
                        return false;
                    };
                    at += ch.len_utf8();
                    if depth == 0 && brackets == 0 {
                        pending_arguments = None;
                        pending_control = false;
                    }
                    continue;
                }
                let name = &prefix[name_start..at];
                if depth == 0 && brackets == 0 {
                    let arguments = tex_table_command_arguments(name);
                    pending_arguments = arguments.filter(|&count| count > 0);
                    pending_control = arguments != Some(0);
                }
                if name == "Verb" {
                    let normalized = format!("\\verb{}", &prefix[at..]);
                    let Some(literal) =
                        crate::editor_math_guides::tex_inline_literal(&normalized, 0)
                    else {
                        return false;
                    };
                    if !literal.closed {
                        return false;
                    }
                    at = start + literal.end;
                    // The shared math reader recognises the lowercase spelling.
                    context_source
                        .to_mut()
                        .replace_range(start + 1..start + 2, "v");
                    if depth == 0 && brackets == 0 {
                        pending_arguments = None;
                        pending_control = false;
                    }
                } else if matches!(name, "begin" | "end") {
                    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                        at += 1;
                    }
                    if bytes.get(at) != Some(&b'{') {
                        return false;
                    }
                    let Some(close) = prefix[at + 1..].find('}') else {
                        return false;
                    };
                    let environment = &prefix[at + 1..at + 1 + close];
                    at += close + 2;
                    if depth == 0 && brackets == 0 {
                        pending_arguments = None;
                        pending_control = name == "begin";
                    }
                    if name == "begin"
                        && matches!(
                            environment,
                            "verbatim"
                                | "verbatim*"
                                | "Verbatim"
                                | "Verbatim*"
                                | "BVerbatim"
                                | "LVerbatim"
                                | "SaveVerbatim"
                                | "lstlisting"
                                | "minted"
                                | "comment"
                                | "filecontents"
                                | "filecontents*"
                        )
                    {
                        let closing = format!("\\end{{{environment}}}");
                        let mut closed = false;
                        for line in prefix[at..].split_inclusive('\n') {
                            at += line.len();
                            if line.trim() == closing {
                                closed = true;
                                break;
                            }
                        }
                        if !closed {
                            return false;
                        }
                        // Preserve offsets while hiding every literal payload
                        // from the shared reader, including fake inline closers.
                        context_source
                            .to_mut()
                            .replace_range(start..at, &" ".repeat(at - start));
                    }
                } else if at == prefix.len()
                    && source[byte..].starts_with(|ch: char| ch.is_ascii_alphabetic())
                {
                    return false;
                }
            }
            _ => {
                let ch = prefix[at..].chars().next().unwrap();
                if depth == 0 && brackets == 0 && !ch.is_whitespace() && ch != '*' {
                    pending_arguments = None;
                    pending_control = false;
                }
                at += ch.len_utf8();
            }
        }
    }
    let mut following = &source[byte..];
    loop {
        following = following.trim_start();
        if following.starts_with('%') {
            let Some(end) = following.find('\n') else {
                break;
            };
            following = &following[end + 1..];
        } else {
            break;
        }
    }
    let following = following
        .strip_prefix('*')
        .unwrap_or(following)
        .trim_start();
    depth == 0
        && brackets == 0
        && pending_arguments.is_none()
        && !(pending_control && following.starts_with(['{', '[']))
        && crate::snippets::tex_context(&context_source, byte) == Some(crate::snippets::Scope::Text)
}

// This is block-insertion admission, not macro expansion. Known multi-argument
// commands must finish their arguments; unknown commands are guarded when an
// explicit next group would otherwise receive the generated \begin token.
fn tex_table_command_arguments(name: &str) -> Option<usize> {
    match name {
        "noindent" | "par" | "centering" | "raggedright" | "raggedleft" | "smallskip"
        | "medskip" | "bigskip" | "newline" | "clearpage" | "newpage" => Some(0),
        "part" | "chapter" | "section" | "subsection" | "subsubsection" | "paragraph"
        | "subparagraph" | "emph" | "textbf" | "textit" | "texttt" | "textrm" | "textsf"
        | "textsc" | "textsl" | "textnormal" | "url" | "nolinkurl" | "footnote" | "caption"
        | "label" | "ref" | "eqref" => Some(1),
        "textcolor" | "colorbox" | "href" => Some(2),
        "fcolorbox" | "resizebox" => Some(3),
        _ => None,
    }
}

fn tex_table_insertion(source: &str, cursor: usize, table: &str) -> Result<String, String> {
    let byte = char_index_to_byte(source, cursor).ok_or("The table range is no longer valid")?;
    if !tex_table_text_context(source, byte) {
        return Err("The table insertion position is no longer document text".into());
    }
    let start = source[..byte].rfind('\n').map_or(0, |at| at + 1);
    let end = source[byte..]
        .find('\n')
        .map_or(source.len(), |at| byte + at);
    let before = &source[start..byte];
    let after = &source[byte..end];
    let indentation = &source[start..end][..source[start..end]
        .bytes()
        .take_while(|ch| matches!(ch, b' ' | b'\t'))
        .count()];
    let indent = if before.bytes().all(|ch| matches!(ch, b' ' | b'\t')) {
        before
    } else {
        indentation
    };
    let newline = if source[..end].ends_with('\r') || source[..start].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut replacement = String::new();
    if !before.trim().is_empty() {
        replacement.push_str(newline);
        replacement.push_str(indent);
    }
    replacement.push_str(
        &table
            .replace("\r\n", "\n")
            .replace('\n', &format!("{newline}{indent}")),
    );
    if !after.trim().is_empty() {
        replacement.push_str(newline);
        replacement.push_str(indent);
    }
    Ok(replacement)
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
        TableEditorDialog::Math(dialog) => {
            if let Some(error) = &dialog.validation {
                return Err(error.clone());
            }
            (
                dialog.key,
                dialog.original.as_str(),
                dialog
                    .table
                    .source_edit()
                    .map_err(|error| error.to_string()),
            )
        }
        TableEditorDialog::Tex(dialog) => {
            if let Some(error) = &dialog.validation {
                return Err(error.clone());
            }
            if dialog.column_spec != dialog.table.column_spec {
                return Err("Finish the column specification before applying the table".into());
            }
            (
                dialog.key,
                dialog.original.as_str(),
                dialog.table.grid_source_edit(),
            )
        }
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
        let replacement = if original.is_empty() && edit.range.is_empty() {
            tex_table_insertion(source, edit.range.start, &edit.replacement)?
        } else {
            edit.replacement
        };
        let cursor = edit.range.start + replacement.chars().count();
        let edit =
            crate::tex::imports::prepare_edit(source, byte_range, &replacement, cursor, &required)?;
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
    match dialog {
        TableEditorDialog::Typst(typst) => show_table_editor_ui(ui, typst, width, height, settings),
        TableEditorDialog::Tex(dialog) => show_tex(ui, dialog, height, settings),
        TableEditorDialog::Math(dialog) => show_math(ui, dialog, height, settings),
    }
}

fn workbench_regions(ui: &egui::Ui, height: f32) -> (Rect, Rect) {
    let available = Rect::from_min_size(
        ui.cursor().min,
        Vec2::new(
            ui.available_width(),
            height.min(ui.available_height()).max(0.0),
        ),
    );
    let footer_top = (available.bottom() - 44.0).max(available.top());
    (
        Rect::from_min_max(
            available.min,
            Pos2::new(available.right(), (footer_top - 4.0).max(available.top())),
        ),
        Rect::from_min_max(Pos2::new(available.left(), footer_top), available.max),
    )
}

fn footer(
    ui: &mut egui::Ui,
    bounds: Rect,
    error: &Option<String>,
    validation: Option<&String>,
) -> Option<TableEditorUiAction> {
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("table-workbench-footer")
            .max_rect(bounds),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(bounds));
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut action = None;
                let apply = table_editor::apply_button(ui, validation.is_none());
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
                if let Some(error) = error.as_ref().or(validation) {
                    ui.add(
                        egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color))
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

fn show_tex(
    ui: &mut egui::Ui,
    dialog: &mut TexDialog,
    height: f32,
    settings: &AppSettings,
) -> Option<TableEditorUiAction> {
    let (body, footer_rect) = workbench_regions(ui, height);
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("tex-table-body")
            .max_rect(body),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(body));
            let locked = crate::table_sampling::view(ui).map_or(0, |view| view.locked.len());
            let mut structural_error = None;
            ui.horizontal_wrapped(|ui| {
                if dialog.table.is_math() {
                    ui.strong("TeX");
                    let mut environment = dialog.table.environment.name();
                    ui.add_enabled_ui(locked == 0, |ui| {
                        egui::ComboBox::from_id_salt("tex-math-environment")
                            .selected_text(environment)
                            .width(112.0)
                            .show_ui(ui, |ui| {
                                for &name in TexTable::math_environment_names() {
                                    ui.selectable_value(&mut environment, name, name);
                                }
                            });
                    });
                    if environment != dialog.table.environment.name() {
                        if let Err(error) = dialog.table.set_math_environment(environment) {
                            structural_error = Some(error);
                        } else {
                            dialog.column_spec = dialog.table.column_spec.clone();
                            dialog.validation = dialog.table.grid_source_edit().err();
                            dialog.error = None;
                        }
                    }
                    let mut rows = dialog.table.cells.as_ref().map_or(1, Vec::len);
                    let mut columns = dialog.table.columns().unwrap_or(1);
                    if math_dimensions(
                        ui,
                        &mut rows,
                        &mut columns,
                        128,
                        dialog.table.fixed_math_columns().is_some(),
                        dialog.table.math_column_step(),
                        locked == 0,
                    ) {
                        if let Err(error) = dialog.table.resize_math_grid(rows, columns) {
                            structural_error = Some(error);
                        } else {
                            dialog.column_spec = dialog.table.column_spec.clone();
                            dialog.validation = dialog.table.grid_source_edit().err();
                            dialog.error = None;
                        }
                    }
                } else {
                    ui.strong(format!("TeX · {}", dialog.table.environment.name()));
                    if let Some(rows) = &dialog.table.cells {
                        ui.weak(format!(
                            "{} × {}",
                            rows.len(),
                            rows.first().map_or(0, Vec::len)
                        ));
                    }
                }
                if locked > 0 {
                    ui.weak(format!("{locked} locked"));
                }
            });
            ui.separator();
            let mut column_spec_changed = false;
            if dialog.table.has_column_spec() {
                ui.horizontal(|ui| {
                    let label = ui.label("Columns");
                    let spec = ui
                        .add_enabled(
                            locked == 0,
                            egui::TextEdit::singleline(&mut dialog.column_spec)
                                .code_editor()
                                .desired_width(ui.available_width()),
                        )
                        .labelled_by(label.id);
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("table.columns", &spec);
                    column_spec_changed = spec.changed();
                });
            }
            let cells_changed = tex_grid(ui, &mut dialog.table, settings);
            refresh_tex_grid_validation(dialog, column_spec_changed || cells_changed);
            if let Some(error) = structural_error {
                dialog.error = Some(error);
            }
        },
    );
    footer(ui, footer_rect, &dialog.error, dialog.validation.as_ref())
}

fn math_dimensions(
    ui: &mut egui::Ui,
    rows: &mut usize,
    columns: &mut usize,
    max_columns: usize,
    fixed_columns: bool,
    column_step: usize,
    enabled: bool,
) -> bool {
    let mut changed = false;
    ui.add_enabled_ui(enabled, |ui| {
        let row_label = ui.label("Rows");
        let row = ui
            .add(egui::DragValue::new(rows).range(1..=4096 / (*columns).max(1)))
            .labelled_by(row_label.id);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("table.rows", &row);
        changed |= row.changed();
        let column_label = ui.label("Columns");
        let column = ui
            .add_enabled(
                !fixed_columns,
                egui::DragValue::new(columns)
                    .range(1..=max_columns.min(4096 / (*rows).max(1)))
                    .speed(column_step as f64),
            )
            .labelled_by(column_label.id);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("table.math_columns", &column);
        changed |= column.changed();
    });
    changed
}

fn resize_typst_math(table: &MathTable, rows: usize, columns: usize) -> Result<MathTable, String> {
    table.source_edit().map_err(|error| error.to_string())?;
    if rows == 0 || columns == 0 || columns > 128 || rows.saturating_mul(columns) > 4096 {
        return Err("The math editor supports up to 4,096 cells and 128 columns".into());
    }
    let mut next = table.clone();
    for _ in next.row_count()..rows {
        next.add_row();
    }
    for _ in rows..next.row_count() {
        next.remove_row();
    }
    for _ in next.columns..columns {
        next.add_column();
    }
    for _ in columns..next.columns {
        next.remove_column();
    }
    if next.row_count() != rows || next.columns != columns {
        return Err("These dimensions are not supported by this math grid".into());
    }
    next.source_edit().map_err(|error| error.to_string())?;
    Ok(next)
}

const MATH_DELIMITERS: &[(&str, &str)] = &[
    ("Default", ""),
    ("( )", "("),
    ("[ ]", "["),
    ("{ }", "{"),
    ("| |", "|"),
    ("‖ ‖", "‖"),
    ("None", "none"),
];

fn show_math(
    ui: &mut egui::Ui,
    dialog: &mut MathDialog,
    height: f32,
    settings: &AppSettings,
) -> Option<TableEditorUiAction> {
    let (body, footer_rect) = workbench_regions(ui, height);
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("typst-math-body")
            .max_rect(body),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(body));
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("Typst · {}", dialog.table.kind.label()));
                let mut rows = dialog.table.row_count();
                let mut columns = dialog.table.columns;
                let maximum = 128;
                if math_dimensions(
                    ui,
                    &mut rows,
                    &mut columns,
                    maximum,
                    dialog.table.kind == MathTableKind::Vector,
                    1,
                    true,
                ) {
                    match resize_typst_math(&dialog.table, rows, columns) {
                        Ok(next) => {
                            dialog.table = next;
                            dialog.error = None;
                        }
                        Err(error) => dialog.error = Some(error),
                    }
                }
                if dialog.table.kind != MathTableKind::Cases {
                    let delimiter = dialog.table.delimiter().unwrap_or_default();
                    let label = MATH_DELIMITERS
                        .iter()
                        .find(|(_, value)| *value == delimiter)
                        .map_or("Custom", |(label, _)| *label);
                    egui::ComboBox::from_id_salt("math-delimiters")
                        .selected_text(label)
                        .width(72.0)
                        .show_ui(ui, |ui| {
                            for &(label, value) in MATH_DELIMITERS {
                                if ui.selectable_label(value == delimiter, label).clicked() {
                                    match dialog.table.set_delimiter(value) {
                                        Ok(()) => dialog.error = None,
                                        Err(error) => dialog.error = Some(error.to_string()),
                                    }
                                }
                            }
                        });
                }
            });
            ui.separator();
            if code_grid(ui, &mut dialog.table, settings) {
                dialog.validation = dialog
                    .table
                    .source_edit()
                    .err()
                    .map(|error| error.to_string());
                dialog.error = None;
            }
        },
    );
    footer(ui, footer_rect, &dialog.error, dialog.validation.as_ref())
}

trait CodeGridDraft {
    fn columns_count(&self) -> usize;
    fn cells(&self) -> &[Vec<String>];
    fn cells_mut(&mut self) -> &mut [Vec<String>];
    fn language(&self) -> &'static str;
    fn is_math(&self) -> bool;
    fn cell_span(&self, _: (usize, usize)) -> (usize, usize) {
        (1, 1)
    }
    fn cell_owner(&self, position: (usize, usize)) -> (usize, usize) {
        position
    }
    fn cell_is_covered(&self, position: (usize, usize)) -> bool {
        self.cell_span(position) == (0, 0)
    }
    fn content_range(&self, (row, column): (usize, usize)) -> Option<std::ops::Range<usize>> {
        Some(0..self.cells().get(row)?.get(column)?.len())
    }
    fn column_label(&self, column: usize) -> String {
        if self.is_math() {
            (column + 1).to_string()
        } else {
            table_editor::column_name(column)
        }
    }
}
impl CodeGridDraft for TexTable {
    fn columns_count(&self) -> usize {
        self.columns().unwrap_or(1).max(1)
    }
    fn cells(&self) -> &[Vec<String>] {
        self.cells.as_deref().expect("admitted TeX grid")
    }
    fn cells_mut(&mut self) -> &mut [Vec<String>] {
        self.cells.as_deref_mut().expect("admitted TeX grid")
    }
    fn language(&self) -> &'static str {
        "tex"
    }
    fn is_math(&self) -> bool {
        TexTable::is_math(self)
    }
    fn cell_span(&self, position: (usize, usize)) -> (usize, usize) {
        TexTable::cell_span(self, position)
    }
    fn cell_owner(&self, position: (usize, usize)) -> (usize, usize) {
        TexTable::cell_owner(self, position)
    }
    fn content_range(&self, position: (usize, usize)) -> Option<std::ops::Range<usize>> {
        self.cell_content_range(position)
    }
    fn column_label(&self, column: usize) -> String {
        if self.fixed_math_columns() == Some(2) {
            if column == 0 {
                "Value".into()
            } else {
                "Condition".into()
            }
        } else if self.is_math() {
            (column + 1).to_string()
        } else {
            table_editor::column_name(column)
        }
    }
}
impl CodeGridDraft for MathTable {
    fn columns_count(&self) -> usize {
        self.columns
    }
    fn cells(&self) -> &[Vec<String>] {
        &self.cells
    }
    fn cells_mut(&mut self) -> &mut [Vec<String>] {
        &mut self.cells
    }
    fn language(&self) -> &'static str {
        "typ"
    }
    fn is_math(&self) -> bool {
        true
    }
    fn column_label(&self, column: usize) -> String {
        match self.kind {
            MathTableKind::Cases if column == 0 => "Value".into(),
            MathTableKind::Cases => "Condition".into(),
            MathTableKind::Vector => "Entry".into(),
            MathTableKind::Matrix => (column + 1).to_string(),
        }
    }
}

fn math_cell_highlight(
    ui: &egui::Ui,
    id: egui::Id,
    source: &str,
    language: &str,
    math: bool,
    settings: &AppSettings,
) -> egui::text::LayoutJob {
    if !math {
        return code_editor::highlight_job(ui, id, source, language, settings);
    }
    let wrapped = format!("${source}$");
    let mut job = code_editor::highlight_job(ui, id, &wrapped, language, settings);
    job.text = source.to_owned();
    job.sections.retain_mut(|section| {
        let start = section.byte_range.start.max(egui::text::ByteIndex(1));
        let end = section
            .byte_range
            .end
            .min(egui::text::ByteIndex(source.len() + 1));
        if start >= end {
            return false;
        }
        section.byte_range = start - 1..end - 1;
        true
    });
    job
}

fn visible_tex_rows(viewport: Rect, row_count: usize) -> std::ops::Range<usize> {
    let first = ((viewport.top() - table_editor::HEADER_HEIGHT) / table_editor::CELL_HEIGHT)
        .floor()
        .max(0.0) as usize;
    let last = ((viewport.bottom() - table_editor::HEADER_HEIGHT) / table_editor::CELL_HEIGHT)
        .ceil()
        .max(0.0) as usize;
    first.min(row_count)..last.saturating_add(1).min(row_count)
}

fn tex_grid_cells(
    table: &impl CodeGridDraft,
    viewport: Rect,
    cell_width: f32,
    focused: Option<(usize, usize)>,
) -> Vec<(usize, usize)> {
    use table_editor::ROW_HEADER;
    let count = table.columns_count();
    let rows = table.cells().len();
    let first = ((viewport.left() - ROW_HEADER) / cell_width)
        .floor()
        .max(0.0) as usize;
    let last = ((viewport.right() - ROW_HEADER) / cell_width)
        .ceil()
        .max(0.0) as usize;
    let mut owners = Vec::new();
    for r in visible_tex_rows(viewport, rows) {
        for c in first.min(count)..last.min(count) {
            owners.push(table.cell_owner((r, c)));
        }
    }
    owners.extend(focused);
    owners.sort_unstable();
    owners.dedup();
    owners
}

fn tex_cell_rect(
    origin: Pos2,
    (row, column): (usize, usize),
    (rows, columns): (usize, usize),
    width: f32,
) -> Rect {
    use table_editor::{CELL_HEIGHT, HEADER_HEIGHT, ROW_HEADER};
    Rect::from_min_size(
        origin
            + Vec2::new(
                ROW_HEADER + column as f32 * width,
                HEADER_HEIGHT + row as f32 * CELL_HEIGHT,
            ),
        Vec2::new(width * columns as f32, CELL_HEIGHT * rows as f32),
    )
}

// Expose only a cell's editable body to TextEdit. Editing the borrowed range
// avoids cloning every visible cell and keeps its TeX wrappers out of selection.
struct CellContents<'a> {
    cell: &'a mut String,
    range: std::ops::Range<usize>,
}

impl egui::TextBuffer for CellContents<'_> {
    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        &self.cell[self.range.clone()]
    }
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<CellContents<'static>>()
    }
    fn insert_text(&mut self, text: &str, index: egui::text::CharIndex) -> usize {
        let offset = self.byte_index_from_char_index(index).0;
        self.cell.insert_str(self.range.start + offset, text);
        self.range.end += text.len();
        text.chars().count()
    }
    fn delete_char_range(&mut self, range: std::ops::Range<egui::text::CharIndex>) {
        let start = self.byte_index_from_char_index(range.start).0;
        let end = self.byte_index_from_char_index(range.end).0;
        self.cell
            .replace_range(self.range.start + start..self.range.start + end, "");
        self.range.end -= end - start;
    }
}

fn tex_grid(ui: &mut egui::Ui, table: &mut TexTable, settings: &AppSettings) -> bool {
    code_grid(ui, table, settings)
}

fn code_grid(ui: &mut egui::Ui, table: &mut impl CodeGridDraft, settings: &AppSettings) -> bool {
    use table_editor::{CELL_HEIGHT, HEADER_HEIGHT, ROW_HEADER};
    let count = table.columns_count();
    let rows = table.cells().len();
    let language = table.language();
    let math = table.is_math();
    let cell_width = ((ui.available_width() - ROW_HEADER - 12.0) / count as f32).max(128.0);
    let width = ROW_HEADER + cell_width * count as f32;
    let cells_id = ui.make_persistent_id("tex-table-cell-editors");
    let focus_id = cells_id.with("focused-cell");
    let mut focused = ui
        .ctx()
        .data(|data| data.get_temp::<(usize, usize)>(focus_id))
        .filter(|&(r, c)| {
            r < rows
                && c < count
                && !table.cell_is_covered((r, c))
                && ui.memory(|memory| memory.focused() == Some(cells_id.with((r, c))))
        });
    let mut navigated = false;
    if math && let Some((r, c)) = focused {
        let direction = ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) {
                -1
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                1
            } else {
                0
            }
        });
        if direction != 0 {
            let total = rows * count;
            let locked = crate::table_sampling::view(ui);
            for step in 1..=total {
                let index = ((r * count + c) as isize + direction * step as isize)
                    .rem_euclid(total as isize) as usize;
                let next = (index / count, index % count);
                if !table.cell_is_covered(next)
                    && !locked
                        .as_ref()
                        .is_some_and(|view| view.locked.contains_key(&next))
                {
                    focused = Some(next);
                    navigated = true;
                    ui.memory_mut(|memory| memory.request_focus(cells_id.with(next)));
                    ui.ctx().data_mut(|data| data.insert_temp(focus_id, next));
                    break;
                }
            }
        }
    }
    let mut changed = false;
    let output = egui::ScrollArea::both()
        .id_salt("tex-table-cells")
        .max_height(ui.available_height().max(1.0))
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let grid_painter = ui.painter().clone();
            let (_, area) =
                ui.allocate_space(Vec2::new(width, HEADER_HEIGHT + rows as f32 * CELL_HEIGHT));
            let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
            let fill = ui.visuals().faint_bg_color;
            let header_fill = ui.visuals().extreme_bg_color;
            let header = Rect::from_min_size(area.min, Vec2::new(width, HEADER_HEIGHT));
            ui.painter().rect_filled(header, 0.0, header_fill);
            let small = egui::TextStyle::Small.resolve(ui.style());
            for c in 0..count {
                let rect = Rect::from_min_size(
                    area.min + Vec2::new(ROW_HEADER + c as f32 * cell_width, 0.0),
                    Vec2::new(cell_width, HEADER_HEIGHT),
                );
                if ui.is_rect_visible(rect) {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        table.column_label(c),
                        small.clone(),
                        ui.visuals().weak_text_color(),
                    );
                    ui.painter()
                        .rect_stroke(rect, 0.0, stroke, StrokeKind::Inside);
                }
            }
            for r in visible_tex_rows(viewport, rows) {
                let number = Rect::from_min_size(
                    area.min + Vec2::new(0.0, HEADER_HEIGHT + r as f32 * CELL_HEIGHT),
                    Vec2::new(ROW_HEADER, CELL_HEIGHT),
                );
                ui.painter().rect_filled(number, 0.0, header_fill);
                ui.painter()
                    .rect_stroke(number, 0.0, stroke, StrokeKind::Inside);
                ui.painter().text(
                    number.center(),
                    egui::Align2::CENTER_CENTER,
                    (r + 1).to_string(),
                    small.clone(),
                    ui.visuals().weak_text_color(),
                );
            }
            for (r, c) in tex_grid_cells(table, viewport, cell_width, focused) {
                let (rowspan, colspan) = table.cell_span((r, c));
                let rect = tex_cell_rect(area.min, (r, c), (rowspan, colspan), cell_width);
                if !ui.is_rect_visible(rect) && focused != Some((r, c)) {
                    continue;
                }
                ui.painter().rect_filled(rect, 0.0, fill);
                ui.painter()
                    .rect_stroke(rect, 0.0, stroke, StrokeKind::Inside);
                if navigated && focused == Some((r, c)) {
                    ui.scroll_to_rect(rect, Some(Align::Center));
                }
                let content_range = table
                    .content_range((r, c))
                    .unwrap_or_else(|| 0..table.cells()[r][c].len());
                let cell = &mut table.cells_mut()[r][c];
                let id = cells_id.with((r, c));
                let inner = rect.shrink2(Vec2::new(6.0, 5.0));
                ui.scope_builder(egui::UiBuilder::new().id_salt(id).max_rect(inner), |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(inner));
                    let mut layouter = |ui: &egui::Ui,
                                        buffer: &dyn egui::TextBuffer,
                                        width: f32| {
                        let mut job =
                            math_cell_highlight(ui, id, buffer.as_str(), language, math, settings);
                        job.wrap.max_width = width;
                        ui.fonts_mut(|fonts| fonts.layout_job(job))
                    };
                    egui::ScrollArea::vertical()
                        .id_salt(("tex-cell-scroll", r, c))
                        .max_height(inner.height())
                        .min_scrolled_height(0.0)
                        .show(ui, |ui| {
                            if table_samples::locked_cell(ui, (r, c), cell).is_some() {
                                return;
                            }
                            let mut contents = CellContents {
                                cell,
                                range: content_range,
                            };
                            let response = ui.add(
                                egui::TextEdit::multiline(&mut contents)
                                    .id(id)
                                    .code_editor()
                                    .frame(egui::Frame::NONE)
                                    .font(theme::editor_font())
                                    .desired_width(inner.width())
                                    .desired_rows(1)
                                    .margin(Vec2::ZERO)
                                    .layouter(&mut layouter),
                            );
                            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                            {
                                let mut visible = response.clone();
                                visible.interact_rect = visible
                                    .interact_rect
                                    .intersect(ui.clip_rect())
                                    .intersect(inner);
                                if visible.interact_rect.is_positive() {
                                    crate::desktop_test::observe(
                                        &format!("table.cell.{r}.{c}"),
                                        &visible,
                                    );
                                }
                            }
                            if response.has_focus() {
                                ui.ctx().data_mut(|data| data.insert_temp(focus_id, (r, c)));
                                grid_painter.rect_stroke(
                                    rect,
                                    0.0,
                                    ui.visuals().selection.stroke,
                                    StrokeKind::Inside,
                                );
                            }
                            changed |= response.changed();
                        });
                });
            }
        });
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe_scroll(
        "table.grid",
        ui.ctx(),
        output.inner_rect,
        output.content_size.y,
        output.state.offset.y,
        false,
    );
    let _ = output;
    changed
}

fn refresh_tex_grid_validation(dialog: &mut TexDialog, changed: bool) {
    // An idle frame must retain unfinished-expression validation without
    // reparsing the complete table.
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
    #[test]
    fn merged_tex_cells_use_one_editor_and_remain_visible_when_the_owner_scrolls_away() {
        let source = r"\begin{tabular}{lll}\multirow{3}{*}{Header} & \multicolumn{2}{c}{Group}\\ & A & B\\ & C & D\\\end{tabular}";
        let table = crate::editor_features::tex_table_at(source, 2).unwrap();
        let viewport = Rect::from_min_size(Pos2::new(0.0, 70.0), Vec2::new(420.0, 50.0));
        let owners = tex_grid_cells(&table, viewport, 128.0, None);
        assert!(
            owners.contains(&(0, 0)),
            "a visible covered row must retain its merged editor"
        );
        assert!(!owners.contains(&(1, 0)) && !owners.contains(&(2, 0)));
        let dialog = TableEditorDialog::Tex(Box::new(TexDialog {
            original: table.source.clone(),
            column_spec: table.column_spec.clone(),
            table,
            key: key(),
            error: None,
            validation: None,
        }));
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(620.0, 420.0))
            .build_ui_state(
                move |ui, dialog| {
                    if !configured {
                        theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        return;
                    }
                    show(ui, dialog, 600.0, 380.0, &AppSettings::default());
                },
                dialog,
            );
        harness.run();
        let fields = harness
            .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
            .collect::<Vec<_>>();
        assert_eq!(
            fields.len(),
            6,
            "merged cells must not create editors at covered positions"
        );
        let header = harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("Header"),
            )
            .rect();
        let group = harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("Group"),
            )
            .rect();
        let a = harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("A"),
            )
            .rect();
        let merged = tex_cell_rect(Pos2::ZERO, (0, 0), (3, 1), 128.0);
        let single = tex_cell_rect(Pos2::ZERO, (1, 1), (1, 1), 128.0);
        assert_eq!(merged.height(), single.height() * 3.0);
        assert_eq!(merged.bottom(), single.bottom() + table_editor::CELL_HEIGHT);
        assert!(
            header.top() < a.top(),
            "the merged owner must remain at its first row"
        );
        assert!(group.width() > a.width() * 1.8, "{group:?} {a:?}");
        assert!(harness.query_by_label("Source").is_none());
    }

    #[test]
    fn tex_visible_owners_are_bounded_in_both_axes_and_retain_focused_input() {
        let body = "A & B & C & D & E & F & G & H \\\\\n".repeat(400);
        let table = crate::editor_features::tex_table_at(
            &format!("\\begin{{tabular}}{{llllllll}}{body}\\end{{tabular}}"),
            2,
        )
        .unwrap();
        let viewport = Rect::from_min_size(Pos2::new(500.0, 5000.0), Vec2::new(300.0, 140.0));
        let owners = tex_grid_cells(&table, viewport, 128.0, Some((0, 0)));
        assert!(owners.contains(&(0, 0)));
        assert!(
            owners.len() <= 28,
            "offscreen cells should not become widgets: {}",
            owners.len()
        );
        for &(r, c) in owners.iter().filter(|&&p| p != (0, 0)) {
            assert!((138..=144).contains(&r), "row {r}");
            assert!((3..=5).contains(&c), "column {c}");
        }
    }

    #[test]
    fn unfinished_column_draft_cannot_be_applied_through_the_commit_boundary() {
        let source = r"\begin{tabular}{ll}A & B\end{tabular}";
        let table = crate::editor_features::tex_table_at(source, 2).unwrap();
        let mut dialog = TexDialog {
            original: table.source.clone(),
            column_spec: "p{".into(),
            table,
            key: key(),
            validation: None,
            error: None,
        };
        assert!(
            prepare_source_edit(
                source,
                key(),
                &TableEditorDialog::Tex(Box::new(dialog.clone()))
            )
            .is_err()
        );
        refresh_tex_grid_validation(&mut dialog, true);
        assert!(dialog.validation.is_some());
        dialog.column_spec = "ll".into();
        refresh_tex_grid_validation(&mut dialog, true);
        assert!(
            prepare_source_edit(source, key(), &TableEditorDialog::Tex(Box::new(dialog))).is_ok()
        );
    }

    #[test]
    fn grid_workbench_never_exposes_a_whole_table_source_mode() {
        let source = r"\begin{tabular}{ll}Alpha & Beta\\\end{tabular}";
        let table = crate::editor_features::tex_table_at(source, 2).unwrap();
        let dialog = TableEditorDialog::Tex(Box::new(TexDialog {
            original: table.source.clone(),
            column_spec: table.column_spec.clone(),
            validation: None,
            table,
            key: key(),
            error: None,
        }));
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(620.0, 420.0))
            .build_ui_state(
                move |ui, dialog| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    show(ui, dialog, 600.0, 380.0, &AppSettings::default());
                },
                dialog,
            );
        harness.run();
        assert!(
            harness.query_by_label("Source").is_none(),
            "table editing must stay in the grid"
        );
        assert!(harness.query_by_label("Apply").is_some());
    }

    fn key() -> DocumentKey {
        DocumentKey {
            owner: tiptoptyp_core::document::WindowSessionId::new(1),
            epoch: 1,
            revision: 2,
        }
    }
    #[test]
    fn new_tex_table_is_an_empty_unicode_safe_grid_at_the_caret() {
        let source =
            "\\documentclass{article}\n\\begin{document}\nPréface α🙂\n  \nFin\n\\end{document}\n";
        let cursor = source[..source.find("  \n").unwrap() + 2].chars().count();
        let draft = new_tex_table_draft(source, cursor, key()).unwrap();
        assert!(draft.original.is_empty());
        assert_eq!(draft.table.source_range, cursor..cursor);
        assert_eq!(draft.table.environment.name(), "tabular");
        assert_eq!(draft.table.columns(), Some(2));
        assert_eq!(
            draft.table.cells.as_ref().unwrap(),
            &vec![vec![String::new(), String::new()]; 2]
        );
        assert!(draft.validation.is_none());
        let dialog = TableEditorDialog::Tex(Box::new(draft));
        let edit = prepare_source_edit(source, key(), &dialog).unwrap();
        let byte = char_index_to_byte(source, cursor).unwrap();
        assert_eq!(edit.byte_range, byte..byte);
        assert!(edit.replacement.starts_with("\\begin{tabular}{ll}\n  "));
        assert!(edit.replacement.ends_with("\n  \\end{tabular}"));
        assert!(!edit.replacement.contains("\\usepackage"));
        let mut applied = source.to_owned();
        applied.replace_range(edit.byte_range, &edit.replacement);
        assert_eq!(&applied[..byte], &source[..byte]);
        assert!(applied.ends_with(&source[byte..]));
        let table = crate::editor_features::tex_table_at(&applied, cursor + 1).unwrap();
        assert_eq!(table.cells.as_ref().unwrap().len(), 2);
        assert_eq!(table.columns(), Some(2));
        let mut stale = key();
        stale.revision += 1;
        assert!(prepare_source_edit(source, stale, &dialog).is_err());
    }

    #[test]
    fn new_tex_table_preserves_crlf_tabs_and_separates_surrounding_prose() {
        let source = "\\begin{document}\r\n\tPréface α🙂 suffix\r\n\\end{document}\r\n";
        let cursor = source[..source.find(" suffix").unwrap()].chars().count();
        let draft = new_tex_table_draft(source, cursor, key()).unwrap();
        let edit =
            prepare_source_edit(source, key(), &TableEditorDialog::Tex(Box::new(draft))).unwrap();
        assert!(
            edit.replacement
                .starts_with("\r\n\t\\begin{tabular}{ll}\r\n\t")
        );
        assert!(edit.replacement.ends_with("\\end{tabular}\r\n\t"));
        assert!(!edit.replacement.replace("\r\n", "").contains('\n'));
        let mut applied = source.to_owned();
        applied.replace_range(edit.byte_range, &edit.replacement);
        assert!(applied.starts_with("\\begin{document}\r\n\tPréface α🙂\r\n"));
        assert!(applied.ends_with("\r\n\t suffix\r\n\\end{document}\r\n"));
    }

    #[test]
    fn new_tex_table_rejects_math_comments_literals_and_command_arguments() {
        for marked in [
            "% a comm§ent\nText",
            r"\verb|a § b|",
            r"\Verb|a § b|",
            r"\lstinline[language=TeX]{a § b}",
            r"\mintinline{tex}|a § b|",
            r"\begin{verbatim}a § b\end{verbatim}",
            r"\begin{Verbatim}a § b\end{Verbatim}",
            r"\begin{comment}a § b\end{comment}",
            r"\begin{minted}{tex}a § b\end{minted}",
            r"\begin{filecontents}{data.txt}a § b\end{filecontents}",
            r"\begin{filecontents*}{data.txt}a § b\end{filecontents*}",
            "\\begin{comment}\nexample \\end{comment} inline\n§literal\n\\end{comment}",
            r"$a § b$",
            r"\[a § b\]",
            r"\begin{equation}a § b\end{equation}",
            r"\begin{align*}a & § b\end{align*}",
            r"$x + \text{a § b}$",
            r"\section{a § b}",
            r"\section §{Title}",
            r"\section*§{Title}",
            "\\section* \n§{Title}",
            r"\textcolor{red} §{Text}",
            r"\href{url} §{Label}",
            r"\unknown §{Argument}",
            r"\unknown§*{Argument}",
            r"\includegraphics§*{figure}",
            r"\rotatebox{90} §{Text}",
            r"\begin{tabular}§{ll}",
            r"\begin{minipage} §{.5\linewidth}",
            r"\section[short § title]{Long title}",
            r"\section[short {nested ] § title}]{Long title}",
            r"\documentclass[twocolumn,§draft]{article}",
            r"\sect§ion{Title}",
            r"\begin{doc§ument}",
            r"{Grouped § text}",
            r"\§text{Title}",
        ] {
            let cursor = marked[..marked.find('§').unwrap()].chars().count();
            let source = marked.replace('§', "");
            assert!(
                new_tex_table_draft(&source, cursor, key()).is_err(),
                "{marked}"
            );
        }
        for marked in [
            "§",
            "Plain §text",
            r"\begin{document} §text\end{document}",
            r"\section{Title} plain §text",
            r"\section{Title}§",
            "\\section{Title}\n§Body",
            r"\textcolor{red}{Text} §body",
            r"\href{url}{Label} §body",
            r"\noindent§",
            r"\noindent§{Grouped paragraph}",
            "\\par§\nBody",
            r"\centering§ \begin{table}",
            r"\section[short {nested ] title}]{Long title} §text",
            r"Escaped \% and \{ brace §text",
            r"\verb|% $ { literal| §text",
            r"\Verb|% $ { literal| §text",
            "\\begin{Verbatim}\n% $ { literal\n\\end{Verbatim}\n§text",
            "\\begin{filecontents}{data.txt}\n% $ { literal\n\\end{filecontents}\n§text",
            "$x$ §text",
            "\\begin{equation}x\\end{equation}\n§Text",
            "% comment with { brace\n§Text",
        ] {
            let cursor = marked[..marked.find('§').unwrap()].chars().count();
            let source = marked.replace('§', "");
            assert!(
                new_tex_table_draft(&source, cursor, key()).is_ok(),
                "{marked}"
            );
        }
    }

    #[test]
    fn new_tex_table_cancel_and_apply_keep_one_undo_including_imports() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let source =
            "\\documentclass{article}\n\\begin{document}\nPréface α🙂\n  \nFin\n\\end{document}\n";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            directory.path().join("new-table.tex"),
            DocumentKind::Tex,
            None,
        );
        let cursor = source[..source.find("  \n").unwrap() + 2].chars().count();
        app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(cursor)));
        let initial_key = app.document().key();
        let initial_history = app.document().history_availability();
        let initial_dirty = app.is_dirty();
        app.begin_new_tex_table(&context);
        assert!(matches!(app.table_editor, Some(TableEditorDialog::Tex(_))));
        assert_eq!(app.document().source(), source);
        assert_eq!(app.document().key(), initial_key);
        app.cancel_table_editor(&context);
        assert_eq!(app.document().source(), source);
        assert_eq!(app.document().key(), initial_key);
        assert_eq!(app.document().history_availability(), initial_history);
        assert_eq!(app.is_dirty(), initial_dirty);

        app.begin_new_tex_table(&context);
        let mut dialog = app.table_editor.take().unwrap();
        let TableEditorDialog::Tex(draft) = &mut dialog else {
            unreachable!()
        };
        draft.table.cells.as_mut().unwrap()[0][0] = r"\textcolor{red}{Été}".into();
        let edit = prepare_source_edit(source, initial_key, &dialog).unwrap();
        app.document_mut()
            .edit(CCursorRange::one(CCursor::new(cursor)), |source| {
                source.replace_range(edit.byte_range, &edit.replacement)
            });
        let applied = app.document().source().clone();
        assert_eq!(applied.matches("\\usepackage{xcolor}").count(), 1);
        assert!(applied.contains(r"\textcolor{red}{Été}"));
        assert!(applied.contains("Préface α🙂\n  \\begin{tabular}"));
        app.undo_editor(&context, false);
        assert_eq!(app.document().source(), source);
        app.undo_editor(&context, true);
        assert_eq!(app.document().source(), &applied);
    }

    #[test]
    fn new_tex_table_rejected_context_does_not_open_a_draft_or_edit_source() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let source = "$x + y$";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            directory.path().join("math.tex"),
            DocumentKind::Tex,
            None,
        );
        app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(3)));
        let key = app.document().key();
        app.begin_new_tex_table(&context);
        assert!(app.table_editor.is_none());
        assert_eq!(app.document().source(), source);
        assert_eq!(app.document().key(), key);
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .message
                .contains("outside comments")
        );
    }

    #[test]
    fn new_tex_table_grid_typing_apply_and_cancel_use_the_normal_controls() {
        struct State {
            dialog: TableEditorDialog,
            source: String,
            action: Option<TableEditorUiAction>,
        }
        for apply in [false, true] {
            let source = "Préface α🙂\n\nFin";
            let cursor = source[..source.find("\n\n").unwrap() + 1].chars().count();
            let dialog = TableEditorDialog::Tex(Box::new(
                new_tex_table_draft(source, cursor, key()).unwrap(),
            ));
            let mut configured = false;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(620.0, 420.0))
                .build_ui_state(
                    move |ui, state: &mut State| {
                        if !configured {
                            theme::configure_test_fonts(ui.ctx());
                            configured = true;
                            return;
                        }
                        if let Some(action) =
                            show(ui, &mut state.dialog, 600.0, 380.0, &AppSettings::default())
                        {
                            if action == TableEditorUiAction::Apply {
                                let edit = prepare_source_edit(&state.source, key(), &state.dialog)
                                    .unwrap();
                                state
                                    .source
                                    .replace_range(edit.byte_range, &edit.replacement);
                            }
                            state.action = Some(action);
                        }
                    },
                    State {
                        dialog,
                        source: source.into(),
                        action: None,
                    },
                );
            harness.run();
            assert_eq!(
                harness
                    .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
                    .count(),
                4
            );
            harness
                .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
                .next()
                .unwrap()
                .click();
            harness.run();
            harness.event(egui::Event::Text("Été".into()));
            harness.run();
            assert_eq!(
                harness.state().source,
                source,
                "typing only changes the draft"
            );
            assert!(harness.query_by_label("Source").is_none());
            harness
                .get_by_label(if apply { "Apply" } else { "Cancel" })
                .click();
            harness.run();
            let state = harness.state();
            assert_eq!(
                state.action,
                Some(if apply {
                    TableEditorUiAction::Apply
                } else {
                    TableEditorUiAction::Cancel
                })
            );
            if apply {
                assert!(state.source.contains("Été"));
                let table =
                    crate::editor_features::tex_table_at(&state.source, cursor + 1).unwrap();
                assert_eq!(table.cells.as_ref().unwrap().len(), 2);
            } else {
                assert_eq!(state.source, source);
            }
        }
    }

    #[test]
    fn tex_grid_apply_is_lossless_and_rejects_stale_source() {
        let source =
            "Préface \\begin{tabular}{|l|r|} A & B \\\\ \\hline C & D \\\\ \\end{tabular} fin";
        let table =
            crate::editor_features::tex_table_at(source, source.find("A &").unwrap()).unwrap();
        let mut dialog = TexDialog {
            original: table.source.clone(),

            column_spec: table.column_spec.clone(),

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
        assert!(prepare_source_edit(&source.replace("A &", "X &"), key(), &grid).is_err());
    }
    #[test]
    fn stale_table_menu_cannot_open_another_document_with_identical_source() {
        for (source, kind) in [
            ("#table([A])", DocumentKind::Typst),
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
    fn package_tex_workbenches_edit_cells_without_a_source_mode() {
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
                original: table.source.clone(),
                column_spec: table.column_spec.clone(),
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
            assert!(harness.query_by_label("Source").is_none());
            let edit = prepare_source_edit(&source, key(), harness.state()).unwrap();
            assert!(edit.replacement.starts_with(&header));
            assert!(edit.replacement.contains('!'));
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
    fn real_arxiv_table_workbench_preserves_exact_edits_cancel_and_undo() {
        let provenance: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/arxiv-tables/provenance.json"
        ))
        .unwrap();
        for (name, fixture) in [
            (
                "scipy-roadmap.tex",
                include_str!("../../tests/fixtures/arxiv-tables/scipy-roadmap.tex"),
            ),
            (
                "scipy-optimizers.tex",
                include_str!("../../tests/fixtures/arxiv-tables/scipy-optimizers.tex"),
            ),
            (
                "platonic-metrics.tex",
                include_str!("../../tests/fixtures/arxiv-tables/platonic-metrics.tex"),
            ),
        ] {
            let metadata = provenance
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["file"] == name)
                .unwrap();
            let before = metadata["edit_before"].as_str().unwrap();
            let after = metadata["edit_after"].as_str().unwrap();
            // A multibyte prefix catches accidental byte/character offset mixing.
            let original = format!("% Préface α🙂\n{fixture}");
            assert_eq!(original.matches(before).count(), 1, "{name}");
            let cursor = original[..original.find(before).unwrap()].chars().count();
            let target = TableTarget::at(&original, cursor, DocumentKind::Tex).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            app.document_mut().replace_loaded_unprojected(
                original.clone(),
                directory.path().join(name),
                DocumentKind::Tex,
                None,
            );
            let initial_key = app.document().key();
            let initial_history = app.document().history_availability();
            let initial_dirty = app.is_dirty();
            app.begin_table_target(target.clone());
            let dialog = app.table_editor.as_ref().unwrap();
            let TableEditorDialog::Tex(tex) = dialog else {
                panic!("{name}: expected TeX workbench");
            };
            assert!(tex.table.cells.is_some(), "{name}: expected editable grid");
            let no_op = prepare_source_edit(&original, initial_key, dialog).unwrap();
            let mut unchanged = original.clone();
            unchanged.replace_range(no_op.byte_range, &no_op.replacement);
            assert_eq!(
                unchanged, original,
                "{name}: untouched draft must be lossless"
            );
            app.cancel_table_editor(&context);
            assert!(app.table_editor.is_none());
            assert_eq!(app.document().source(), &original, "{name}: cancel source");
            assert_eq!(app.document().key(), initial_key, "{name}: cancel revision");
            assert_eq!(app.document().history_availability(), initial_history);
            assert_eq!(app.is_dirty(), initial_dirty, "{name}: cancel dirty state");

            app.begin_table_target(target);
            let mut dialog = app.table_editor.take().unwrap();
            let TableEditorDialog::Tex(tex) = &mut dialog else {
                unreachable!();
            };
            let cell = tex
                .table
                .cells
                .as_mut()
                .unwrap()
                .iter_mut()
                .flatten()
                .find(|cell| cell.contains(before))
                .unwrap();
            *cell = cell.replacen(before, after, 1);
            let mut stale = initial_key;
            stale.revision += 1;
            assert!(
                prepare_source_edit(&original, stale, &dialog).is_err(),
                "{name}"
            );
            let edit = prepare_source_edit(&original, initial_key, &dialog).unwrap();
            let expected = original.replacen(before, after, 1);
            app.document_mut()
                .edit(CCursorRange::one(CCursor::new(cursor)), |source| {
                    source.replace_range(edit.byte_range, &edit.replacement)
                });
            // Exact full-source equality also guards every preamble import and
            // macro definition against duplication, rewriting or removal.
            assert_eq!(app.document().source(), &expected, "{name}: one exact edit");
            app.undo_editor(&context, false);
            assert_eq!(app.document().source(), &original, "{name}: one exact undo");
            assert!(
                !app.document().history_availability().0,
                "{name}: one edit only"
            );
            app.undo_editor(&context, true);
            assert_eq!(app.document().source(), &expected, "{name}: exact redo");
        }
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
    fn compact_tex_row_range_is_bounded_and_respects_the_column_header() {
        use table_editor::{CELL_HEIGHT, HEADER_HEIGHT};
        let view = |top| Rect::from_min_size(Pos2::new(0.0, top), Vec2::new(500.0, 400.0));
        assert_eq!(visible_tex_rows(view(0.0), 400).start, 0);
        let middle = visible_tex_rows(view(HEADER_HEIGHT + 200.0 * CELL_HEIGHT), 400);
        assert_eq!(middle.start, 200);
        assert!(middle.len() <= (400.0 / CELL_HEIGHT).ceil() as usize + 2);
        assert_eq!(visible_tex_rows(view(100_000.0), 400), 400..400);
        assert_eq!(visible_tex_rows(view(0.0), 0), 0..0);
    }

    #[test]
    fn tex_workbench_actions_stay_labelled_and_pinned_with_long_errors() {
        let table =
            crate::editor_features::tex_table_at("\\begin{tabular}{ll}A & B\\end{tabular}", 2)
                .unwrap();
        let dialogs = [TableEditorDialog::Tex(Box::new(TexDialog {
            original: table.source.clone(),
            column_spec: table.column_spec.clone(),

            table,
            key: key(),
            error: Some("Table error with details ".repeat(50)),
            validation: None,
        }))];
        for dialog in dialogs {
            let mut ready = false;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(440.0, 260.0))
                .build_ui_state(
                    move |ui, state| {
                        if !ready {
                            theme::configure_test_fonts(ui.ctx());
                            ready = true;
                            return;
                        }
                        let height = ui.available_height();
                        if let Some(action) = show(
                            ui,
                            &mut state.0,
                            ui.available_width(),
                            height,
                            &AppSettings::default(),
                        ) {
                            state.1 = Some(action);
                        }
                    },
                    (dialog, None::<TableEditorUiAction>),
                );
            harness.run();
            for label in ["Apply", "Cancel"] {
                let rect = harness
                    .get_by_role_and_label(egui::accesskit::Role::Button, label)
                    .rect();
                assert!(
                    rect.top() >= 200.0,
                    "{label} must stay at the bottom: {rect:?}"
                );
                assert!(
                    rect.bottom() <= 260.0 && rect.right() <= 440.0,
                    "{label} escaped the viewport: {rect:?}"
                );
            }
            harness.get_by_label("Cancel").click();
            harness.run();
            assert_eq!(harness.state().1, Some(TableEditorUiAction::Cancel));
        }
    }

    #[test]
    fn large_tex_grid_renders_visible_rows_and_keeps_footer_available() {
        let source = format!(
            "\\begin{{tabular}}{{ll}}{}\\end{{tabular}}",
            "A & B \\\\\n".repeat(400)
        );
        let table = crate::editor_features::tex_table_at(&source, 2).unwrap();
        let dialog = TableEditorDialog::Tex(Box::new(TexDialog {
            original: table.source.clone(),
            column_spec: table.column_spec.clone(),
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
            original: table.source.clone(),
            column_spec: "l>{unfinished".into(),
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
    fn scrolling_tex_rows_far_past_the_focused_cell_keeps_receiving_text() {
        let source = format!(
            "\\begin{{tabular}}{{ll}}{}\\end{{tabular}}",
            (0..40)
                .map(|row| format!("row-{row} & other-{row} \\\\\n"))
                .collect::<String>()
        );
        let table = crate::editor_features::tex_table_at(&source, 2).unwrap();
        let dialog = TableEditorDialog::Tex(Box::new(TexDialog {
            original: table.source.clone(),
            column_spec: table.column_spec.clone(),
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
                delta: Vec2::new(0.0, -900.0),
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
        let retained = harness.get(
            by().role(egui::accesskit::Role::MultilineTextInput)
                .value("row-4"),
        );
        assert!(
            retained.rect().bottom() < 0.0,
            "the focused editor must be retained after it leaves the viewport"
        );
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
    fn cell_contents_edits_unicode_without_touching_wrappers() {
        use egui::TextBuffer as _;
        use egui::text::CharIndex;
        let mut raw = r"\multirow{2}{*}{\textbf{Énergie}}".to_owned();
        let start = raw.find('É').unwrap();
        let mut body = CellContents {
            range: start..start + "Énergie".len(),
            cell: &mut raw,
        };
        assert_eq!(body.as_str(), "Énergie");
        body.delete_char_range(CharIndex(0)..CharIndex(1));
        body.insert_text("Σ", CharIndex(0));
        assert_eq!(body.as_str(), "Σnergie");
        body.replace_with("温度");
        assert_eq!(body.as_str(), "温度");
        assert_eq!(raw, r"\multirow{2}{*}{\textbf{温度}}");
    }

    #[test]
    fn merged_tex_grid_exposes_cell_content_and_preserves_formatting_on_edit() {
        let source = r"\begin{tabular}{ll}\multicolumn{2}{c}{\textbf{Metric}}\\\end{tabular}";
        let table = crate::editor_features::tex_table_at(source, 1).unwrap();
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(860.0, 560.0))
            .build_ui_state(
                move |ui, dialog| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    show(ui, dialog, 830.0, 520.0, &AppSettings::default());
                },
                TableEditorDialog::Tex(Box::new(TexDialog {
                    original: table.source.clone(),
                    column_spec: table.column_spec.clone(),
                    table,
                    key: key(),
                    error: None,
                    validation: None,
                })),
            );
        harness.run();
        let field = harness.get(
            by().role(egui::accesskit::Role::MultilineTextInput)
                .value("Metric"),
        );
        field.click();
        harness.run();
        harness.event(egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        });
        harness.event(egui::Event::Text("Résumé".into()));
        harness.run();
        assert!(
            harness
                .query(
                    by().role(egui::accesskit::Role::MultilineTextInput)
                        .value("Résumé")
                )
                .is_some()
        );
        // Incomplete typing must stay inside the same projected body across
        // frames, without revealing wrappers or shifting the caret into them.
        harness.event(egui::Event::ModifiersChanged(Modifiers::NONE));
        for (text, expected, valid) in [
            ("{", "Résumé{", false),
            ("π", "Résumé{π", false),
            ("}", "Résumé{π}", true),
        ] {
            harness.event(egui::Event::Text(text.into()));
            harness.run();
            assert!(
                harness
                    .query(
                        by().role(egui::accesskit::Role::MultilineTextInput)
                            .value(expected)
                    )
                    .is_some()
            );
            assert_eq!(
                !harness.get_by_label("Apply").accesskit_node().is_disabled(),
                valid
            );
        }
        let edit = prepare_source_edit(source, key(), harness.state()).unwrap();
        assert_eq!(edit.replacement, source.replace("Metric", "Résumé{π}"));
    }

    #[test]
    fn grid_cell_child_keyboard_pastes_real_tex_after_select_all() {
        struct State {
            dialog: TableEditorDialog,
            queued: Vec<egui::Event>,
            allow_requested_paste: bool,
            requested_paste_admitted: bool,
        }
        let source = include_str!("../../tests/fixtures/arxiv-tables/platonic-metrics.tex");
        let table = crate::editor_features::tex_table_at(
            source,
            source[..source.find("Unbiased estimator of CKA").unwrap()]
                .chars()
                .count(),
        )
        .unwrap();
        let original = table.cells.as_ref().unwrap()[3][5].clone();
        let content_range = table.cell_content_range((3, 5)).unwrap();
        let editor_text = original[content_range].to_owned();
        let edited_editor_text = editor_text.replacen(
            "Unbiased estimator of CKA",
            "Bias-corrected estimator of CKA",
            1,
        );
        let edited = original.replacen(
            "Unbiased estimator of CKA",
            "Bias-corrected estimator of CKA",
            1,
        );
        for (modifiers, allow_requested_paste) in [
            (Modifiers::COMMAND, false),
            (Modifiers::NONE, true),
            // Without a physical chord or an explicit RequestPaste receipt,
            // semantic clipboard input is intentionally not admitted.
            (Modifiers::NONE, false),
        ] {
            let mut fonts_ready = false;
            let settings = AppSettings::default();
            let shortcuts = settings.effective_shortcuts();
            let mut harness = Harness::builder()
                .with_size(Vec2::new(1600.0, 560.0))
                .build_ui_state(
                    move |ui, state: &mut State| {
                        if !fonts_ready {
                            theme::configure_test_fonts(ui.ctx());
                            fonts_ready = true;
                            return;
                        }
                        // Keep the same normalization and event lifetimes as
                        // the native persistent table child, before its field.
                        let physical = prepare_child_keyboard(
                            ui,
                            &mut state.dialog,
                            &shortcuts,
                            state.allow_requested_paste,
                        );
                        state.requested_paste_admitted |= physical.requested_paste_admitted;
                        let _queued = crate::child_view::begin_queued_edit_events(
                            ui.ctx(),
                            &mut state.queued,
                        );
                        show(ui, &mut state.dialog, 1570.0, 520.0, &settings);
                    },
                    State {
                        dialog: TableEditorDialog::Tex(Box::new(TexDialog {
                            original: original.clone(),

                            column_spec: table.column_spec.clone(),
                            validation: None,
                            table: table.clone(),
                            key: key(),
                            error: None,
                        })),
                        queued: Vec::new(),
                        allow_requested_paste,
                        requested_paste_admitted: false,
                    },
                );
            harness.run();
            let field = harness
                .get(
                    by().role(egui::accesskit::Role::MultilineTextInput)
                        .value(&editor_text),
                )
                .rect();
            // A long source field's geometric center lies below the clipped
            // viewport. Click the visible first line, as the user does.
            let point = Pos2::new(field.left() + 24.0, field.top() + 8.0);
            harness.input_mut().events.extend([
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                },
            ]);
            harness.run();
            let focused = harness.ctx.memory(|memory| memory.focused()).unwrap();
            harness.state_mut().queued.push(egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            });
            harness.step();
            let selected = egui::text_edit::TextEditState::load(&harness.ctx, focused)
                .unwrap()
                .cursor
                .char_range()
                .unwrap()
                .as_sorted_char_range();
            assert_eq!(
                selected.start.0..selected.end.0,
                0..editor_text.chars().count()
            );
            harness.input_mut().events.extend([
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Paste(edited_editor_text.clone()),
            ]);
            harness.step();
            let TableEditorDialog::Tex(dialog) = &harness.state().dialog else {
                unreachable!();
            };
            let expected = if modifiers.command || allow_requested_paste {
                &edited
            } else {
                &original
            };
            assert!(
                dialog.table.cells.as_ref().unwrap()[3][5] == *expected,
                "Grid cell paste must obey physical/requested ownership: {modifiers:?}, requested={allow_requested_paste}, got {} bytes, expected {}",
                dialog.table.cells.as_ref().unwrap()[3][5].len(),
                expected.len(),
            );
            assert!(dialog.validation.is_none());
            assert!(harness.state().queued.is_empty());
            assert_eq!(harness.ctx.memory(|memory| memory.focused()), Some(focused));
            if allow_requested_paste {
                assert!(harness.state().requested_paste_admitted);
            }
        }
    }
}

#[cfg(test)]
#[path = "math_workbench_tests.rs"]
mod math_tests;
