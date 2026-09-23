//! Read-only editor Git decorations. Views receive immutable snapshots and
//! emit typed actions; document and repository mutation stays in controllers.
use super::{ChunkDiff, marker_geometry};
use crate::{
    git::{
        repository::{
            diff::{Hunk, LineChangeCounts},
            hunks::Action as HunkAction,
        },
        view::show_colored_diff,
    },
    settings::GitDiffStyle,
    shortcuts::ShortcutBindings,
};
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    OpenChunk(usize),
    RunHunk(HunkAction),
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
            let label = change.label();
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
            let color = change.kind.color(ui.ctx());
            if geometry.paint.is_positive() {
                ui.painter().rect_filled(geometry.paint, 1.0, color);
            }
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Open hunk diff")
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
    toolbar_style: crate::settings::ToolbarStyle,
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
                let shortcut = shortcuts.display(action.shortcut()).unwrap_or_default();
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
                    !busy,
                    false,
                    icon,
                    action.label(),
                    toolbar_style,
                    false,
                )
                .on_hover_text(action.label());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, !busy, action.label())
                });
                let response = if shortcut.is_empty() {
                    response
                } else {
                    response.on_hover_text(format!("{} · {shortcut}", action.label()))
                };
                if response.clicked() {
                    selected = Some(Action::RunHunk(action));
                }
            }
        });
    });
    ui.add(egui::Label::new(chunk.path.to_string_lossy()).truncate())
        .on_hover_text(chunk.path.display().to_string());
    ui.weak("Selected chunk, including unsaved edits at the time it was opened.");
    ui.separator();
    egui::ScrollArea::both()
        .id_salt("git-chunk-text")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            show_colored_diff(ui, &chunk.hunk.text, diff_style);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        git::repository::diff::{ChangeKind, LineChange},
        shortcuts::{ShortcutBindings, ShortcutPlatform},
    };
    use egui_kittest::{Harness, kittest::Queryable as _};

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
                        crate::settings::ToolbarStyle::Icons,
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
