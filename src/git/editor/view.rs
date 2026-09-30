//! Read-only editor Git decorations. Views receive immutable snapshots and
//! emit typed actions; document and repository mutation stays in controllers.
use super::{ChunkDiff, marker_geometry};
use crate::{
    git::{
        repository::{
            diff::{Hunk, LineChangeCounts},
            hunks::Action as HunkAction,
        },
        view::show_colored_diff_with_lines,
    },
    settings::GitDiffStyle,
    shortcuts::ShortcutBindings,
};
use eframe::egui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    OpenChunk(usize),
    RunHunk(HunkAction),
    Close,
    ApplyText(String),
}

pub(crate) fn show_markers(
    ui: &mut egui::Ui,
    hunks: &[Hunk],
    rows: &[egui::Rect],
    gutter_left: f32,
) -> Option<Action> {
    let mut selected = None;
    for (index, hunk) in hunks.iter().enumerate() {
        for (part, change) in hunk.changes.iter().enumerate() {
            let Some(geometry) = marker_geometry(change, rows, gutter_left, ui.clip_rect()) else {
                continue;
            };
            let response = ui.interact(
                geometry.hit,
                ui.id().with(("git-hunk", index, part)),
                egui::Sense::click(),
            );
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            if ui.is_rect_visible(response.rect) {
                crate::desktop_test::observe(&format!("git.hunk.{index}"), &response);
            }
            let label = change.label();
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
            let color = change.kind.color(ui.ctx());
            if geometry.paint.is_positive() {
                ui.painter().rect_filled(geometry.paint, 1.0, color);
            }
            if crate::app::git_hunk_hover(
                response.on_hover_cursor(egui::CursorIcon::PointingHand),
                &hunk.text,
            )
            .clicked()
            {
                selected = Some(Action::OpenChunk(index));
            }
        }
    }
    selected
}

pub(crate) fn show_chunk(
    ui: &mut egui::Ui,
    chunk: &ChunkDiff,
    shortcuts: &ShortcutBindings,
    busy: bool,
    diff_style: GitDiffStyle,
    settings: &crate::settings::AppSettings,
    draft: &mut super::draft::Draft,
) -> Option<Action> {
    let mut selected = None;
    ui.horizontal(|ui| {
        ui.add_sized(
            [0.0, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new("Changes since last commit").strong()),
        );
        ui.separator();
        show_change_counts(
            ui,
            LineChangeCounts::from_hunks(std::slice::from_ref(&chunk.hunk)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for action in HunkAction::ALL.into_iter().rev() {
                let tooltip = match action {
                    HunkAction::Previous => "Previous hunk and show its diff".to_owned(),
                    HunkAction::Next => "Next hunk and show its diff".to_owned(),
                    _ => shortcuts.display(action.shortcut()).map_or_else(
                        || action.label().to_owned(),
                        |key| format!("{} · {key}", action.label()),
                    ),
                };
                use crate::app::icons::{self, UiIcon};
                let icon = match action {
                    HunkAction::Stage => UiIcon::Stage,
                    HunkAction::Unstage => UiIcon::Unstage,
                    HunkAction::Revert => UiIcon::Revert,
                    HunkAction::Previous => UiIcon::Previous,
                    HunkAction::Next => UiIcon::Next,
                };
                let response = icons::toolbar_button(
                    ui,
                    !busy && !draft.changed(),
                    false,
                    icon,
                    action.label(),
                    settings.toolbar_style,
                    false,
                )
                .on_hover_text(tooltip);
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, !busy, action.label())
                });
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe(&format!("git.hunk.{}", action.label()), &response);
                if response.clicked() {
                    selected = Some(Action::RunHunk(action));
                }
            }
        });
    });
    ui.add(egui::Label::new(chunk.path.to_string_lossy()).truncate())
        .on_hover_text(chunk.path.display().to_string());
    ui.horizontal(|ui| {
        let diff = ui.selectable_value(&mut draft.editing, false, "Diff");
        let edit = ui.selectable_value(&mut draft.editing, true, "Edit");
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        {
            crate::desktop_test::observe("git.hunk.Diff", &diff);
            crate::desktop_test::observe("git.hunk.Edit", &edit);
        }
        #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
        let _ = (diff, edit);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close = ui.button("×");
            close.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close hunk editor")
            });
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("git.hunk.Close", &close);
            if close.clicked() {
                selected = Some(Action::Close);
            }
            let apply = ui.add_enabled(!busy, egui::Button::new("Apply"));
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("git.hunk.Apply", &apply);
            if apply.clicked() {
                selected = Some(Action::ApplyText(draft.text.clone()));
            }
        });
    });
    ui.separator();
    egui::ScrollArea::both()
        .id_salt("git-chunk-text")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if draft.editing {
                ui.columns(2, |columns| {
                    columns[0].strong("Previous");
                    let mut previous: String = draft
                        .rows
                        .iter()
                        .filter(|row| row.kind != '+')
                        .map(|row| row.text.as_str())
                        .collect();
                    let (old_line, new_line) = crate::git::view::hunk_line_starts(
                        chunk.hunk.text.lines().next().unwrap_or_default(),
                    )
                    .unwrap_or((1, 1));
                    let extension = chunk
                        .path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .unwrap_or("txt");
                    crate::app::code_editor::source_editor(
                        &mut columns[0],
                        ("hunk-before", &chunk.path),
                        &mut previous,
                        extension,
                        settings,
                        crate::app::code_editor::Options {
                            rows: 12,
                            start_line: old_line,
                            editable: false,
                            frame: Some(egui::Frame::NONE),
                        },
                    );
                    columns[1].strong("Current");
                    let edit = crate::app::code_editor::source_editor(
                        &mut columns[1],
                        ("hunk-after", &chunk.path),
                        &mut draft.text,
                        extension,
                        settings,
                        crate::app::code_editor::Options {
                            rows: 12,
                            start_line: new_line,
                            editable: !busy,
                            frame: Some(egui::Frame::NONE),
                        },
                    );
                    edit.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::TextEdit,
                            !busy,
                            "Hunk contents",
                        )
                    });
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("git.hunk.editor", &edit);
                    if edit.changed() {
                        draft.manual = true;
                    }
                });
            } else {
                let mut revert = None;
                show_colored_diff_with_lines(
                    ui,
                    &chunk.hunk.text,
                    diff_style,
                    |ui, index, rect| {
                        let Some(row) = draft.rows.get(index) else {
                            return;
                        };
                        let enabled = !busy && !draft.manual;
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(rect.right() - 20.0, rect.top()),
                            egui::vec2(20.0, 20.0),
                        );
                        let response = ui.interact(
                            rect,
                            ui.id().with(("revert-line", index)),
                            if enabled {
                                egui::Sense::click()
                            } else {
                                egui::Sense::hover()
                            },
                        );
                        if response.hovered() || row.reverted {
                            ui.painter().rect_filled(rect, 2.0, ui.visuals().panel_fill);
                            crate::app::icons::paint_ui_icon(
                                ui.painter(),
                                rect.shrink(4.0),
                                crate::app::icons::UiIcon::Revert,
                                ui.style().interact(&response).fg_stroke.color,
                            );
                        }
                        let label = if row.kind == '-' {
                            "Restore removed line"
                        } else {
                            "Remove added line"
                        };
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                enabled,
                                format!("{label} {}", index + 1),
                            )
                        });
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        if ui.is_rect_visible(rect) {
                            crate::desktop_test::observe(
                                &format!("git.hunk.line.{index}"),
                                &response,
                            );
                        }
                        if enabled && response.clicked() {
                            revert = Some(index);
                        }
                    },
                );
                if let Some(index) = revert {
                    draft.toggle_line(index);
                }
            }
        });
    selected
}

fn show_change_counts(ui: &mut egui::Ui, counts: LineChangeCounts) {
    let palette = crate::theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = crate::theme::SPACE.small;
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

/// Compact read-only counterpart to the docked editor.
pub(crate) fn show_hover_diff(ui: &mut egui::Ui, diff: &str) {
    ui.strong("Changes since last commit");
    show_colored_diff_with_lines(ui, diff, GitDiffStyle::SideBySide, |_, _, _| {});
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        git::repository::diff::{ChangeKind, LineChange},
        shortcuts::{ShortcutBindings, ShortcutPlatform},
    };
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn hover_diff_is_read_only_and_keeps_paired_lines() {
        let mut harness = Harness::new_ui(|ui| show_hover_diff(ui, "@@ -1 +1 @@\n-old\n+new\n"));
        harness.run();
        harness.get_by_label("Changes since last commit");
        assert!(harness.query_by_label("Apply").is_none());
        assert!(harness.query_by_label("Edit").is_none());
    }

    #[test]
    fn line_revert_controls_keep_positions_and_apply_the_draft() {
        let chunk = ChunkDiff {
            path: "/project/main.typ".into(),
            hunk: Hunk {
                text: "@@ -1 +1 @@\n-old\n+new\n".into(),
                changes: vec![],
            },
        };
        let initial = (
            super::super::draft::Draft::new(&chunk.hunk).unwrap(),
            None::<Action>,
        );
        let mut harness = Harness::builder()
            .with_size(egui::vec2(620.0, 420.0))
            .build_ui_state(
                move |ui, state| {
                    if let Some(action) = show_chunk(
                        ui,
                        &chunk,
                        &ShortcutBindings::defaults(ShortcutPlatform::current()),
                        false,
                        GitDiffStyle::Unified,
                        &crate::settings::AppSettings {
                            toolbar_style: crate::settings::ToolbarStyle::Icons,
                            ..Default::default()
                        },
                        &mut state.0,
                    ) {
                        state.1 = Some(action);
                    }
                },
                initial,
            );
        harness.run();
        let position = harness.get_by_label("Remove added line 2").rect();
        harness.get_by_label("Restore removed line 1").click();
        harness.run();
        assert_eq!(position, harness.get_by_label("Remove added line 2").rect());
        harness.get_by_label("Remove added line 2").click();
        harness.run();
        harness.get_by_label("Apply").click();
        harness.run();
        assert_eq!(harness.state().1, Some(Action::ApplyText("old\n".into())));
    }

    #[test]
    fn hunk_actions_share_the_compact_title_row_and_emit_a_typed_action() {
        let chunk = ChunkDiff {
            path: "/project/main.typ".into(),
            hunk: Hunk {
                text: "@@ -1 +1 @@\n-old\n+new\n".into(),
                changes: vec![LineChange {
                    lines: 0..1,
                    kind: ChangeKind::Modified,
                    old_line_count: 1,
                    new_line_count: 1,
                }],
            },
        };
        let shortcuts = ShortcutBindings::defaults(ShortcutPlatform::current());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(620.0, 260.0))
            .build_ui_state(
                move |ui, selected| {
                    if let Some(action) = show_chunk(
                        ui,
                        &chunk,
                        &shortcuts,
                        false,
                        GitDiffStyle::Unified,
                        &crate::settings::AppSettings {
                            toolbar_style: crate::settings::ToolbarStyle::Icons,
                            ..Default::default()
                        },
                        &mut super::super::draft::Draft::new(&chunk.hunk).unwrap(),
                    ) {
                        *selected = Some(action);
                    }
                },
                None::<Action>,
            );
        harness.run();
        let title = harness.get_by_label("Changes since last commit").rect();
        for label in ["Previous", "Next", "Stage", "Unstage", "Revert"] {
            let button = harness.get_by_label(label).rect();
            assert!(
                (button.center().y - title.center().y).abs() < 1.0,
                "{label}: {button:?} vs {title:?}"
            );
            assert!(
                button.height() < 24.0,
                "{label} was not compact: {button:?}"
            );
        }
        harness.get_by_label("Revert").click();
        harness.run();
        assert_eq!(*harness.state(), Some(Action::RunHunk(HunkAction::Revert)));
    }
}
