//! Read-only Git presentation. Rendering emits typed actions and has no access
//! to repository handles, workers, or the filesystem.
use super::DiffView;
use crate::{
    git::repository::{DiffKind, DiffSelection, Entry, Operation, Snapshot},
    theme,
};
use eframe::egui;
use std::{path::PathBuf, sync::Arc};

pub(super) struct Input<'a> {
    pub(super) snapshot: &'a Snapshot,
    pub(super) message: &'a str,
    pub(super) commit_message: &'a str,
    pub(super) failed: bool,
    pub(super) diff: Option<&'a DiffView>,
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
}

#[derive(Debug)]
struct DiffLayout {
    content: Arc<str>,
    style: DiffStyle,
    font_cache: Arc<egui::Galley>,
    galley: Arc<egui::Galley>,
}

#[derive(Debug, PartialEq)]
struct DiffStyle {
    font: egui::FontId,
    colors: [egui::Color32; 4],
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
        }
    }

    fn layout_job(&self, content: &str) -> egui::text::LayoutJob {
        let mut layout = egui::text::LayoutJob::default();
        for line in content.split_inclusive('\n') {
            let color = self.colors[if line.starts_with('+') {
                0
            } else if line.starts_with('-') {
                1
            } else if line.starts_with("@@") {
                2
            } else {
                3
            }];
            layout.append(
                line,
                0.0,
                egui::TextFormat {
                    font_id: self.font.clone(),
                    color,
                    ..Default::default()
                },
            );
        }
        layout
    }
}

impl Cache {
    pub(super) fn diff_galley(&mut self, ui: &egui::Ui, content: &Arc<str>) -> Arc<egui::Galley> {
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
            && Arc::ptr_eq(&layout.font_cache, &font_cache)
        {
            return Arc::clone(&layout.galley);
        }
        let galley = ui.painter().layout_job(style.layout_job(content));
        self.diff_layout = Some(DiffLayout {
            content: Arc::clone(content),
            style,
            font_cache,
            galley: Arc::clone(&galley),
        });
        galley
    }
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
                show_diff(ui, input.diff, cache, &mut output);
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

fn show_diff(ui: &mut egui::Ui, diff: Option<&DiffView>, cache: &mut Cache, output: &mut Output) {
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
                });
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(diff.selection.path.to_string_lossy()).monospace(),
                )
                .truncate(),
            )
            .on_hover_text(diff.selection.path.display().to_string());
            ui.weak(diff.selection.kind.description());
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
                    egui::ScrollArea::both()
                        .id_salt((
                            "git-diff",
                            &diff.selection.path,
                            diff.selection.kind == DiffKind::Staged,
                        ))
                        .max_height(230.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            let galley = cache.diff_galley(ui, &content.text);
                            ui.add(egui::Label::new(galley).selectable(true).extend());
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

pub(crate) fn show_colored_diff(ui: &mut egui::Ui, content: &str) {
    let layout = DiffStyle::from_ui(ui).layout_job(content);
    ui.add(egui::Label::new(layout).selectable(true).extend());
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

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
