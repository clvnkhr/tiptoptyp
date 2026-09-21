//! Read-only Git presentation. Rendering emits typed actions and has no access
//! to repository handles, workers, or the filesystem.
use super::DiffView;
use crate::{
    git::repository::{
        DiffKind, DiffSelection, Entry, Operation, Snapshot,
        diff::{LineChangeCounts, parse_hunks},
    },
    settings::GitDiffStyle,
    theme,
};
use eframe::egui;
use std::{ops::Range, path::PathBuf, sync::Arc};

pub(super) struct Input<'a> {
    pub(super) snapshot: &'a Snapshot,
    pub(super) message: &'a str,
    pub(super) commit_message: &'a str,
    pub(super) failed: bool,
    pub(super) diff: Option<&'a DiffView>,
    pub(super) diff_style: GitDiffStyle,
    pub(super) busy: bool,
    pub(super) dirty: bool,
}

#[derive(Default)]
pub(crate) struct Output {
    pub(super) operation: Option<Operation>,
    pub(super) commit_message: Option<String>,
    pub(super) revealed: Option<DiffSelection>,
    #[cfg(test)]
    pub(super) rendered_change_rows: usize,
}

#[derive(Default)]
pub(super) struct Cache {
    commit_message: String,
    diff_layout: Option<DiffLayout>,
    diff_model: Option<DiffModel>,
    diff_counts: Option<DiffCounts>,
}

#[derive(Debug)]
struct DiffLayout {
    content: Arc<str>,
    style: DiffStyle,
    max_width: f32,
    font_cache: Arc<egui::Galley>,
    galley: Arc<egui::Galley>,
}

#[derive(Debug, PartialEq)]
struct DiffStyle {
    font: egui::FontId,
    colors: [egui::Color32; 4],
    backgrounds: [egui::Color32; 4],
    changed_backgrounds: [egui::Color32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLineKind {
    Added,
    Removed,
    Hunk,
    Context,
    Meta,
}

#[derive(Debug, Clone)]
struct DiffCell {
    text: String,
    kind: DiffLineKind,
    line: Option<usize>,
    changed: Option<Range<usize>>,
}

#[derive(Debug, Clone)]
struct DiffRow {
    full: Option<DiffCell>,
    left: Option<DiffCell>,
    right: Option<DiffCell>,
}

#[derive(Debug)]
struct DiffModel {
    content: Arc<str>,
    rows: Vec<DiffRow>,
}

#[derive(Debug)]
struct DiffCounts {
    content: Arc<str>,
    counts: LineChangeCounts,
}

impl DiffStyle {
    fn from_ui(ui: &egui::Ui) -> Self {
        let palette = theme::palette(ui.ctx());
        Self {
            font: egui::TextStyle::Monospace.resolve(ui.style()),
            colors: [
                palette.success,
                palette.error,
                palette.info,
                ui.visuals().text_color(),
            ],
            backgrounds: [
                palette.success.gamma_multiply(0.16),
                palette.error.gamma_multiply(0.16),
                palette.info.gamma_multiply(0.14),
                egui::Color32::TRANSPARENT,
            ],
            changed_backgrounds: [
                palette.success.gamma_multiply(0.40),
                palette.error.gamma_multiply(0.40),
                palette.info.gamma_multiply(0.26),
                egui::Color32::TRANSPARENT,
            ],
        }
    }

    fn color(&self, kind: DiffLineKind) -> egui::Color32 {
        self.colors[match kind {
            DiffLineKind::Added => 0,
            DiffLineKind::Removed => 1,
            DiffLineKind::Hunk => 2,
            DiffLineKind::Context | DiffLineKind::Meta => 3,
        }]
    }

    fn background(&self, kind: DiffLineKind) -> egui::Color32 {
        self.backgrounds[match kind {
            DiffLineKind::Added => 0,
            DiffLineKind::Removed => 1,
            DiffLineKind::Hunk => 2,
            DiffLineKind::Context | DiffLineKind::Meta => 3,
        }]
    }

    fn changed_background(&self, kind: DiffLineKind) -> egui::Color32 {
        self.changed_backgrounds[match kind {
            DiffLineKind::Added => 0,
            DiffLineKind::Removed => 1,
            DiffLineKind::Hunk => 2,
            DiffLineKind::Context | DiffLineKind::Meta => 3,
        }]
    }

    fn layout_job(&self, content: &str) -> egui::text::LayoutJob {
        let mut layout = egui::text::LayoutJob::default();
        for line in content.split_inclusive('\n') {
            let kind = diff_line_kind(line);
            layout.append(
                line,
                0.0,
                egui::TextFormat {
                    font_id: self.font.clone(),
                    color: self.color(kind),
                    background: self.background(kind),
                    ..Default::default()
                },
            );
        }
        layout
    }
}

impl Cache {
    pub(super) fn diff_galley(
        &mut self,
        ui: &egui::Ui,
        content: &Arc<str>,
        max_width: f32,
    ) -> Arc<egui::Galley> {
        let style = DiffStyle::from_ui(ui);
        // As with viewport_fonts, an empty layout witnesses egui's font-cache
        // lifetime. Fonts, density, and atlas resets must invalidate retained
        // galleys even if the logical FontId stayed the same.
        let font_cache = ui.fonts_mut(|fonts| {
            fonts.layout_no_wrap(String::new(), egui::FontId::default(), egui::Color32::WHITE)
        });
        if let Some(layout) = &self.diff_layout
            && Arc::ptr_eq(&layout.content, content)
            && layout.style == style
            && layout.max_width == max_width
            && Arc::ptr_eq(&layout.font_cache, &font_cache)
        {
            return Arc::clone(&layout.galley);
        }
        let mut job = style.layout_job(content);
        job.wrap.max_width = max_width;
        job.wrap.break_anywhere = true;
        let galley = ui.painter().layout_job(job);
        self.diff_layout = Some(DiffLayout {
            content: Arc::clone(content),
            style,
            max_width,
            font_cache,
            galley: Arc::clone(&galley),
        });
        galley
    }

    fn diff_model(&mut self, content: &Arc<str>) -> &DiffModel {
        let replace = self
            .diff_model
            .as_ref()
            .is_none_or(|model| !Arc::ptr_eq(&model.content, content));
        if replace {
            let rows = side_by_side_rows(content);
            self.diff_model = Some(DiffModel {
                content: Arc::clone(content),
                rows,
            });
        }
        self.diff_model
            .as_ref()
            .expect("diff model was inserted above")
    }

    fn diff_counts(&mut self, content: &Arc<str>) -> LineChangeCounts {
        let replace = self
            .diff_counts
            .as_ref()
            .is_none_or(|cached| !Arc::ptr_eq(&cached.content, content));
        if replace {
            let counts = parse_hunks(content)
                .map(|hunks| LineChangeCounts::from_hunks(&hunks))
                .unwrap_or_default();
            self.diff_counts = Some(DiffCounts {
                content: Arc::clone(content),
                counts,
            });
        }
        self.diff_counts
            .as_ref()
            .expect("diff counts were inserted above")
            .counts
    }
}

fn diff_line_kind(line: &str) -> DiffLineKind {
    if line.starts_with("@@") {
        DiffLineKind::Hunk
    } else if line.starts_with('+') {
        DiffLineKind::Added
    } else if line.starts_with('-') {
        DiffLineKind::Removed
    } else {
        DiffLineKind::Context
    }
}

fn line_body(line: &str) -> &str {
    line.strip_suffix('\n')
        .unwrap_or(line)
        .strip_suffix('\r')
        .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line))
}

fn diff_cell(line: &str, kind: DiffLineKind, strip_prefix: bool) -> DiffCell {
    let text = if strip_prefix {
        line.get(1..).unwrap_or_default()
    } else {
        line
    };
    DiffCell {
        text: line_body(text).to_owned(),
        kind,
        line: None,
        changed: None,
    }
}

fn side_by_side_rows(content: &str) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut in_hunk = false;
    let mut old_line = 0;
    let mut new_line = 0;
    let mut removed = Vec::<DiffCell>::new();
    let mut added = Vec::<DiffCell>::new();

    let flush_changes =
        |rows: &mut Vec<DiffRow>, removed: &mut Vec<DiffCell>, added: &mut Vec<DiffCell>| {
            let count = removed.len().max(added.len());
            for index in 0..count {
                let mut left = removed.get(index).cloned();
                let mut right = added.get(index).cloned();
                match (&mut left, &mut right) {
                    (Some(left), Some(right)) => {
                        let (left_changed, right_changed) =
                            intra_line_change_ranges(&left.text, &right.text);
                        left.changed = left_changed;
                        right.changed = right_changed;
                    }
                    (Some(left), None) | (None, Some(left)) => {
                        if !left.text.is_empty() {
                            left.changed = Some(0..left.text.len());
                        }
                    }
                    (None, None) => {}
                }
                rows.push(DiffRow {
                    full: None,
                    left,
                    right,
                });
            }
            removed.clear();
            added.clear();
        };

    for raw in content.split_inclusive('\n') {
        let line = line_body(raw);
        if line.starts_with("@@") {
            flush_changes(&mut rows, &mut removed, &mut added);
            rows.push(DiffRow {
                full: Some(diff_cell(line, DiffLineKind::Hunk, false)),
                left: None,
                right: None,
            });
            if let Some((old_start, new_start)) = hunk_line_starts(line) {
                old_line = old_start;
                new_line = new_start;
            }
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            rows.push(DiffRow {
                full: Some(diff_cell(line, DiffLineKind::Meta, false)),
                left: None,
                right: None,
            });
            continue;
        }
        match line.as_bytes().first().copied() {
            Some(b'-') => {
                removed.push(DiffCell {
                    text: line.get(1..).unwrap_or_default().to_owned(),
                    kind: DiffLineKind::Removed,
                    line: Some(old_line),
                    changed: None,
                });
                old_line = old_line.saturating_add(1);
            }
            Some(b'+') => {
                added.push(DiffCell {
                    text: line.get(1..).unwrap_or_default().to_owned(),
                    kind: DiffLineKind::Added,
                    line: Some(new_line),
                    changed: None,
                });
                new_line = new_line.saturating_add(1);
            }
            Some(b' ') => {
                flush_changes(&mut rows, &mut removed, &mut added);
                let context = line.get(1..).unwrap_or_default().to_owned();
                let cell = DiffCell {
                    text: context,
                    kind: DiffLineKind::Context,
                    line: Some(old_line),
                    changed: None,
                };
                old_line = old_line.saturating_add(1);
                let right = DiffCell {
                    line: Some(new_line),
                    ..cell.clone()
                };
                new_line = new_line.saturating_add(1);
                rows.push(DiffRow {
                    full: None,
                    left: Some(cell.clone()),
                    right: Some(right),
                });
            }
            Some(b'\\') => {
                flush_changes(&mut rows, &mut removed, &mut added);
                rows.push(DiffRow {
                    full: Some(diff_cell(line, DiffLineKind::Meta, false)),
                    left: None,
                    right: None,
                });
            }
            _ => {
                flush_changes(&mut rows, &mut removed, &mut added);
                rows.push(DiffRow {
                    full: Some(diff_cell(line, DiffLineKind::Meta, false)),
                    left: None,
                    right: None,
                });
            }
        }
    }
    flush_changes(&mut rows, &mut removed, &mut added);
    rows
}

fn hunk_line_starts(line: &str) -> Option<(usize, usize)> {
    let mut fields = line.split_whitespace();
    fields.next()?;
    let old = fields.next()?.strip_prefix('-')?;
    let new = fields.next()?.strip_prefix('+')?;
    let start = |range: &str| {
        range
            .split_once(',')
            .map_or(range, |(start, _)| start)
            .parse::<usize>()
            .ok()
    };
    Some((start(old)?, start(new)?))
}

fn intra_line_change_ranges(
    left: &str,
    right: &str,
) -> (Option<Range<usize>>, Option<Range<usize>>) {
    let left_chars = left.chars().collect::<Vec<_>>();
    let right_chars = right.chars().collect::<Vec<_>>();
    let prefix = left_chars
        .iter()
        .zip(&right_chars)
        .take_while(|(left, right)| left == right)
        .count();
    let max_suffix = left_chars
        .len()
        .saturating_sub(prefix)
        .min(right_chars.len().saturating_sub(prefix));
    let suffix = (0..max_suffix)
        .take_while(|offset| {
            left_chars[left_chars.len() - 1 - offset] == right_chars[right_chars.len() - 1 - offset]
        })
        .count();
    let range = |text: &str, char_count: usize| {
        let start = text
            .char_indices()
            .nth(prefix)
            .map_or(text.len(), |(index, _)| index);
        let end_char = char_count.saturating_sub(suffix);
        let end = text
            .char_indices()
            .nth(end_char)
            .map_or(text.len(), |(index, _)| index);
        (start < end).then_some(start..end)
    };
    (
        range(left, left_chars.len()),
        range(right, right_chars.len()),
    )
}

fn show_change_counts(ui: &mut egui::Ui, counts: LineChangeCounts) {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SPACE.small;
        for (symbol, count, color) in [
            ("+", counts.added, palette.success),
            ("~", counts.modified, palette.info),
            ("-", counts.deleted, palette.error),
        ] {
            if count > 0 {
                ui.colored_label(color, format!("{symbol}{count}"));
            }
        }
    });
}

fn show_side_by_side_diff(ui: &mut egui::Ui, rows: &[DiffRow], style: &DiffStyle) {
    let available_width = diff_available_width(ui).max(2.0);
    let gap = ui.spacing().item_spacing.x.min(available_width / 4.0);
    let line_height = ui
        .text_style_height(&egui::TextStyle::Monospace)
        .max(ui.spacing().interact_size.y);
    let line_number_width = 42.0;
    let column_width = ((available_width - gap) / 2.0).max(1.0);
    let total_width = column_width * 2.0 + gap;
    let horizontal_padding = 2.0 * theme::SPACE.small;
    let side_text_width = (column_width - line_number_width - horizontal_padding).max(1.0);
    let full_text_width = (total_width - horizontal_padding).max(1.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for label in ["Previous", "Current"] {
            ui.add_sized(
                [column_width, line_height],
                egui::Label::new(egui::RichText::new(label).small().strong())
                    .halign(egui::Align::Min),
            );
        }
    });
    for row in rows {
        if let Some(cell) = &row.full {
            let galley = diff_cell_galley(ui, cell, style, full_text_width);
            let row_height = galley.size().y.max(line_height);
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(total_width, row_height), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 0.0, style.background(cell.kind));
            show_diff_cell(ui, rect, cell, 0.0, galley, style);
            continue;
        }
        let left_galley = row
            .left
            .as_ref()
            .map(|cell| diff_cell_galley(ui, cell, style, side_text_width));
        let right_galley = row
            .right
            .as_ref()
            .map(|cell| diff_cell_galley(ui, cell, style, side_text_width));
        let row_height = left_galley
            .as_ref()
            .map_or(line_height, |galley| galley.size().y.max(line_height))
            .max(
                right_galley
                    .as_ref()
                    .map_or(line_height, |galley| galley.size().y.max(line_height)),
            );
        let (row_rect, _) =
            ui.allocate_exact_size(egui::vec2(total_width, row_height), egui::Sense::hover());
        let left_rect =
            egui::Rect::from_min_size(row_rect.min, egui::vec2(column_width, row_height));
        let right_rect = egui::Rect::from_min_size(
            egui::pos2(row_rect.left() + column_width + gap, row_rect.top()),
            egui::vec2(column_width, row_height),
        );
        for (cell, rect, galley) in [
            (row.left.as_ref(), left_rect, left_galley),
            (row.right.as_ref(), right_rect, right_galley),
        ] {
            if let Some(cell) = cell {
                ui.painter()
                    .rect_filled(rect, 0.0, style.background(cell.kind));
                show_diff_cell(ui, rect, cell, line_number_width, galley.unwrap(), style);
            } else {
                ui.painter()
                    .rect_filled(rect, 0.0, style.background(DiffLineKind::Context));
            }
        }
    }
}

fn diff_available_width(ui: &egui::Ui) -> f32 {
    ui.available_width().min(ui.clip_rect().width()).max(1.0)
}

fn diff_cell_galley(
    ui: &egui::Ui,
    cell: &DiffCell,
    style: &DiffStyle,
    max_width: f32,
) -> Arc<egui::Galley> {
    let mut job = diff_cell_job(cell, style);
    job.wrap.max_width = max_width;
    job.wrap.break_anywhere = true;
    ui.painter().layout_job(job)
}

fn diff_cell_job(cell: &DiffCell, style: &DiffStyle) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    if let Some(changed) = &cell.changed {
        if changed.start > 0 {
            job.append(
                &cell.text[..changed.start],
                0.0,
                egui::TextFormat {
                    font_id: style.font.clone(),
                    color: style.color(cell.kind),
                    ..Default::default()
                },
            );
        }
        job.append(
            &cell.text[changed.clone()],
            0.0,
            egui::TextFormat {
                font_id: style.font.clone(),
                color: style.color(cell.kind),
                background: style.changed_background(cell.kind),
                italics: true,
                ..Default::default()
            },
        );
        if changed.end < cell.text.len() {
            job.append(
                &cell.text[changed.end..],
                0.0,
                egui::TextFormat {
                    font_id: style.font.clone(),
                    color: style.color(cell.kind),
                    ..Default::default()
                },
            );
        }
    } else {
        job.append(
            &cell.text,
            0.0,
            egui::TextFormat {
                font_id: style.font.clone(),
                color: style.color(cell.kind),
                ..Default::default()
            },
        );
    }
    job
}

fn show_diff_cell(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    cell: &DiffCell,
    line_number_width: f32,
    galley: Arc<egui::Galley>,
    style: &DiffStyle,
) {
    let inner = rect.shrink2(egui::vec2(theme::SPACE.small, 0.0));
    let number = cell.line.map_or_else(String::new, |line| line.to_string());
    if !number.is_empty() {
        let number_galley = ui.painter().layout_no_wrap(
            number,
            egui::TextStyle::Monospace.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
        ui.painter()
            .galley(inner.min, number_galley, ui.visuals().weak_text_color());
    }
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(
            (inner.left() + line_number_width).min(inner.right()),
            inner.top(),
        ),
        inner.max,
    );
    ui.painter()
        .galley(text_rect.left_top(), galley, style.color(cell.kind));
}

pub(super) fn show_panel(ui: &mut egui::Ui, input: Input<'_>, cache: &mut Cache) -> Output {
    if cache.commit_message != input.commit_message {
        cache.commit_message.clear();
        cache.commit_message.push_str(input.commit_message);
    }
    let mut output = Output::default();
    let palette = theme::palette(ui.ctx());
    egui::ScrollArea::vertical()
        .id_salt("git-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_enabled_ui(!input.busy, |ui| {
                right_action_row(ui, |ui| {
                    if input.snapshot.initialized {
                        for (label, hint, operation) in [
                            (
                                "Push",
                                "Send local commits to the configured remote.",
                                Operation::Push,
                            ),
                            (
                                "Pull",
                                "Fetch and fast-forward the current branch. Divergent branches are left unchanged.",
                                Operation::Pull,
                            ),
                            (
                                "Fetch",
                                "Download remote updates without changing working files.",
                                Operation::Fetch,
                            ),
                        ] {
                            if ui
                                .add_enabled(!input.dirty, egui::Button::new(label))
                                .on_hover_text(hint)
                                .clicked()
                            {
                                output.operation = Some(operation);
                            }
                        }
                    } else if ui.button("Initialize repository").clicked() {
                        output.operation = Some(Operation::Init);
                    }
                    ui.with_layout(
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&input.snapshot.branch).strong(),
                                )
                                .truncate(),
                            );
                        },
                    );
                });
            });
            if input.dirty {
                ui.colored_label(
                    palette.warning,
                    "Save editor changes before staging or committing. Git uses files on disk.",
                );
            }
            ui.add_space(theme::SPACE.content);
            ui.separator();
            if input.snapshot.initialized {
                let staged = input.snapshot.entries.staged;
                ui.add_enabled_ui(!input.busy, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Changes");
                        ui.weak(format!(
                            "{} files · {staged} staged",
                            input.snapshot.entries.len()
                        ));
                    });
                    right_action_row(ui, |ui| {
                        if ui
                            .add_enabled(staged > 0, egui::Button::new("Unstage all"))
                            .on_hover_text("Remove all changes from the staging area. Keep all working files and edits.")
                            .clicked()
                        {
                            output.operation = Some(Operation::UnstageAll);
                        }
                        if ui
                            .add_enabled(
                                !input.dirty && input.snapshot.entries.stageable,
                                egui::Button::new("Stage all"),
                            )
                            .on_hover_text("Stage all working changes, excluding .tiptoptyp temporary files.")
                            .clicked()
                        {
                            output.operation = Some(Operation::StageAll);
                        }
                    });
                    ui.add_space(theme::SPACE.small);
                    egui::ScrollArea::vertical()
                        .id_salt("git-changes")
                        .max_height(192.0)
                        .auto_shrink([false, true])
                        .show_rows(
                            ui,
                            change_row_height(ui),
                            input.snapshot.entries.len(),
                            |ui, rows| {
                                let buttons = ChangeButtons::for_ui(ui);
                                for row in rows {
                                    let entry = &input.snapshot.entries[row];
                                    #[cfg(test)]
                                    {
                                        output.rendered_change_rows += 1;
                                    }
                                    let selected = input.diff.is_some_and(|diff| {
                                        diff.selection.path == entry.path
                                    });
                                    let fill = if selected {
                                        palette.active_row
                                    } else if row % 2 == 0 {
                                        ui.visuals().faint_bg_color
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };
                                    egui::Frame::new()
                                        .fill(fill)
                                        .inner_margin(egui::Margin::symmetric(0, 4))
                                        .show(ui, |ui| {
                                            ui.push_id(&entry.path, |ui| {
                                                right_action_row(ui, |ui| {
                                                    if let Some(operation) =
                                                        buttons.show(ui, entry, input.dirty)
                                                    {
                                                        output.operation = Some(operation);
                                                    }
                                                    ui.with_layout(
                                                        egui::Layout::left_to_right(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.add_space(theme::SPACE.small);
                                                            ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(format!(
                                                                        "{}{}",
                                                                        entry.index,
                                                                        entry.worktree
                                                                    ))
                                                                    .monospace()
                                                                    .color(if entry.staged() {
                                                                        palette.success
                                                                    } else {
                                                                        palette.warning
                                                                    }),
                                                                ),
                                                            )
                                                            .on_hover_text("Git status: staging area / working file. A added, M modified, D deleted, ? untracked.");
                                                            ui.add(
                                                                egui::Label::new(
                                                                    entry.path.to_string_lossy(),
                                                                )
                                                                .truncate(),
                                                            )
                                                            .on_hover_text(
                                                                entry.path.display().to_string(),
                                                            );
                                                        },
                                                    );
                                                });
                                            });
                                        });
                                }
                                if input.snapshot.entries.is_empty() {
                                    ui.add_space(theme::SPACE.content);
                                    ui.colored_label(palette.success, "Working tree clean");
                                    ui.add_space(theme::SPACE.content);
                                }
                            },
                        );
                });
                if input.snapshot.entries.staged_private {
                    ui.colored_label(palette.warning, "Temporary .tiptoptyp files are already staged. Unstage all keeps these files out of the next commit.");
                }
                ui.add_space(theme::SPACE.content);
                show_diff(ui, input.diff, input.diff_style, cache, &mut output);
                ui.add_space(theme::SPACE.content);
                ui.separator();
                let response = ui.add_enabled(
                    !input.busy,
                    egui::TextEdit::multiline(&mut cache.commit_message)
                        .hint_text("Describe your changes…")
                        .desired_width(f32::INFINITY)
                        .desired_rows(2),
                );
                if response.changed() {
                    output.commit_message = Some(cache.commit_message.clone());
                }
                right_action_row(ui, |ui| {
                    if ui
                        .add_enabled(
                            !input.busy
                                && !input.dirty
                                && staged > 0
                                && !cache.commit_message.trim().is_empty(),
                            egui::Button::new("Commit staged changes"),
                        )
                        .clicked()
                    {
                        output.operation = Some(Operation::Commit(cache.commit_message.clone()));
                    }
                });
                egui::CollapsingHeader::new("Recent commits").show(ui, |ui| {
                    ui.monospace(&input.snapshot.history);
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if input.busy {
                    ui.spinner();
                }
                ui.add(
                    egui::Label::new(egui::RichText::new(input.message).color(if input.failed {
                        palette.error
                    } else {
                        palette.neutral
                    }))
                    .selectable(true),
                );
            });
        });
    output
}

fn show_diff(
    ui: &mut egui::Ui,
    diff: Option<&DiffView>,
    diff_style: GitDiffStyle,
    cache: &mut Cache,
    output: &mut Output,
) {
    let Some(diff) = diff else {
        return;
    };
    let palette = theme::palette(ui.ctx());
    let response = egui::Frame::group(ui.style())
        .inner_margin(theme::SPACE.content)
        .show(ui, |ui| {
            right_action_row(ui, |ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.strong(diff.selection.kind.title());
                    ui.separator();
                    if let Some(Ok(content)) = &diff.content {
                        show_change_counts(ui, cache.diff_counts(&content.text));
                    }
                });
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(diff.selection.path.to_string_lossy()).monospace(),
                )
                .truncate(),
            )
            .on_hover_text(diff.selection.path.display().to_string());
            ui.horizontal_wrapped(|ui| {
                ui.weak(diff.selection.kind.description());
                ui.separator();
                ui.weak(diff_style.label());
            });
            ui.separator();
            match &diff.content {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading diff…");
                    });
                }
                Some(Err(error)) => {
                    ui.colored_label(palette.error, error.as_str());
                }
                Some(Ok(content)) if content.text.is_empty() => {
                    ui.label(diff.selection.kind.empty_message());
                }
                Some(Ok(content)) => {
                    egui::ScrollArea::vertical()
                        .id_salt((
                            "git-diff",
                            &diff.selection.path,
                            diff.selection.kind == DiffKind::Staged,
                        ))
                        .max_height(230.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| match diff_style {
                            GitDiffStyle::Unified => {
                                let galley =
                                    cache.diff_galley(ui, &content.text, diff_available_width(ui));
                                ui.add(egui::Label::new(galley).selectable(true).extend());
                            }
                            GitDiffStyle::SideBySide => {
                                let style = DiffStyle::from_ui(ui);
                                let model = cache.diff_model(&content.text);
                                show_side_by_side_diff(ui, &model.rows, &style);
                            }
                        });
                }
            }
        })
        .response;
    if diff.reveal {
        response.scroll_to_me(Some(egui::Align::Center));
        output.revealed = Some(diff.selection.clone());
    }
}

/// Reserve action columns from the right before allowing left-hand text to
/// consume the remaining width. Long paths can never displace the buttons.
fn right_action_row(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), contents);
    });
}

fn change_row_height(ui: &egui::Ui) -> f32 {
    (ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y)
        .max(ui.text_style_height(&egui::TextStyle::Body))
        .max(ui.text_style_height(&egui::TextStyle::Monospace))
        .max(ui.spacing().interact_size.y)
        .max(24.0)
        + 8.0
}

struct ChangeButtons {
    widths: [f32; 2],
    abbreviated: bool,
}

impl ChangeButtons {
    fn for_ui(ui: &egui::Ui) -> Self {
        let font = egui::TextStyle::Button.resolve(ui.style());
        let width = |label: &str| {
            ui.painter()
                .layout_no_wrap(label.into(), font.clone(), ui.visuals().text_color())
                .size()
                .x
                + ui.spacing().button_padding.x * 2.0
        };
        let full = [
            width("Unstage").max(width("Stage")),
            width("Staged diff").max(width("Diff")),
        ];
        let abbreviated = ui.available_width()
            < full.iter().sum::<f32>() + 128.0 + 2.0 * ui.spacing().item_spacing.x;
        let compact = width("S").max(width("U")).max(width("D")).max(24.0);
        Self {
            widths: if abbreviated { [compact; 2] } else { full },
            abbreviated,
        }
    }

    fn show(&self, ui: &mut egui::Ui, entry: &Entry, dirty: bool) -> Option<Operation> {
        let staged = entry.staged();
        let actions = if staged {
            [
                (
                    "Unstage",
                    true,
                    "Keep the working file and remove its staged changes.",
                    Operation::Unstage as fn(PathBuf) -> Operation,
                ),
                (
                    "Staged diff",
                    true,
                    "Show changes staged for the next commit.",
                    |path| Operation::Diff(path, DiffKind::Staged),
                ),
            ]
        } else {
            [
                (
                    "Stage",
                    !dirty && entry.stageable(),
                    "Stage this file's working changes for the next commit.",
                    Operation::Stage as fn(PathBuf) -> Operation,
                ),
                (
                    "Diff",
                    entry.unstaged(),
                    "Show this file's unstaged working changes.",
                    |path| Operation::Diff(path, DiffKind::WorkingTree),
                ),
            ]
        };
        let mut selected = None;
        for ((label, enabled, hint, operation), width) in actions.into_iter().zip(self.widths) {
            let text = if self.abbreviated { &label[..1] } else { label };
            let response = ui.add_enabled(
                enabled,
                egui::Button::new(text).min_size(egui::vec2(width, 24.0)),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label)
            });
            if response.on_hover_text(format!("{label}: {hint}")).clicked() {
                selected = Some(operation(entry.path.clone()));
            }
        }
        selected
    }
}

pub(crate) fn show_colored_diff(ui: &mut egui::Ui, content: &str, diff_style: GitDiffStyle) {
    let style = DiffStyle::from_ui(ui);
    match diff_style {
        GitDiffStyle::Unified => {
            let layout = style.layout_job(content);
            ui.add(egui::Label::new(layout).selectable(true).extend());
        }
        GitDiffStyle::SideBySide => {
            let rows = side_by_side_rows(content);
            show_side_by_side_diff(ui, &rows, &style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn side_by_side_rows_pair_replacements_and_keep_git_headers_full_width() {
        let rows = side_by_side_rows(
            "diff --git a/main.typ b/main.typ\n--- a/main.typ\n+++ b/main.typ\n@@ -1,3 +1,3 @@\n context\n-old\n+new\n tail\n",
        );
        assert_eq!(
            rows[0].full.as_ref().map(|cell| cell.kind),
            Some(DiffLineKind::Meta)
        );
        assert_eq!(
            rows[3].full.as_ref().map(|cell| cell.kind),
            Some(DiffLineKind::Hunk)
        );
        let replacement = rows
            .iter()
            .find(|row| {
                row.left
                    .as_ref()
                    .is_some_and(|cell| cell.kind == DiffLineKind::Removed)
            })
            .expect("replacement row");
        assert_eq!(replacement.left.as_ref().unwrap().text, "old");
        assert_eq!(replacement.right.as_ref().unwrap().text, "new");
        assert_eq!(replacement.left.as_ref().unwrap().line, Some(2));
        assert_eq!(replacement.right.as_ref().unwrap().line, Some(2));
        assert_eq!(
            replacement.left.as_ref().unwrap().kind,
            DiffLineKind::Removed
        );
        assert_eq!(
            replacement.right.as_ref().unwrap().kind,
            DiffLineKind::Added
        );
    }

    #[test]
    fn hunk_line_starts_accept_single_line_ranges() {
        assert_eq!(hunk_line_starts("@@ -8 +12 @@"), Some((8, 12)));
        assert_eq!(hunk_line_starts("@@ -8,2 +12,4 @@"), Some((8, 12)));
        assert_eq!(hunk_line_starts("not a hunk"), None);
    }

    #[test]
    fn intra_line_change_ranges_ignore_shared_prefix_and_suffix() {
        assert_eq!(
            intra_line_change_ranges("keep old value", "keep new value"),
            (Some(5..8), Some(5..8))
        );
        assert_eq!(intra_line_change_ranges("same", "same"), (None, None));
        assert_eq!(
            intra_line_change_ranges("old", "new text"),
            (Some(0..3), Some(0..8))
        );
    }

    #[test]
    fn long_diff_cells_wrap_to_their_available_width() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(320.0, 200.0))
            .build_ui_state(
                |ui, heights: &mut Option<(f32, f32)>| {
                    let style = DiffStyle::from_ui(ui);
                    let cell = DiffCell {
                        text: "a-really-long-unbroken-source-token-that-must-wrap".to_owned(),
                        kind: DiffLineKind::Added,
                        line: Some(1),
                        changed: None,
                    };
                    let narrow = diff_cell_galley(ui, &cell, &style, 80.0);
                    let wide = diff_cell_galley(ui, &cell, &style, 800.0);
                    *heights = Some((narrow.size().y, wide.size().y));
                },
                None,
            );
        harness.run();
        let (narrow, wide) = harness.state().expect("layout heights");
        assert!(
            narrow > wide,
            "narrow diff cells should wrap: {narrow} <= {wide}"
        );
    }

    #[test]
    fn read_only_panel_view_emits_a_typed_action_without_an_io_handle() {
        let snapshot = Snapshot {
            branch: "main".into(),
            initialized: true,
            ..Default::default()
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(500.0, 300.0))
            .build_ui_state(
                move |ui, state: &mut (Cache, Option<Operation>)| {
                    let output = show_panel(
                        ui,
                        Input {
                            snapshot: &snapshot,
                            message: "Ready",
                            commit_message: "",
                            failed: false,
                            diff: None,
                            diff_style: GitDiffStyle::Unified,
                            busy: false,
                            dirty: false,
                        },
                        &mut state.0,
                    );
                    if output.operation.is_some() {
                        state.1 = output.operation;
                    }
                },
                (Cache::default(), None),
            );
        harness.run();
        harness.get_by_label("Fetch").click();
        harness.run();
        assert!(matches!(harness.state().1, Some(Operation::Fetch)));
    }
}
