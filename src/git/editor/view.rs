//! Read-only editor Git decorations. Views receive immutable snapshots and
//! emit typed actions; document and repository mutation stays in controllers.
use super::marker_geometry;
use crate::{
    git::{repository::diff::Hunk, view::show_colored_diff_with_lines},
    settings::GitDiffStyle,
};
use eframe::egui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    OpenChunk(usize),
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

/// Compact read-only counterpart to the docked editor.
pub(crate) fn show_hover_diff(ui: &mut egui::Ui, diff: &str) {
    ui.strong("Changes since last commit");
    show_colored_diff_with_lines(ui, diff, GitDiffStyle::SideBySide, |_, _, _| {});
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn hover_diff_is_read_only_and_keeps_paired_lines() {
        let mut harness = Harness::new_ui(|ui| show_hover_diff(ui, "@@ -1 +1 @@\n-old\n+new\n"));
        harness.run();
        harness.get_by_label("Changes since last commit");
        assert!(harness.query_by_label("Apply").is_none());
        assert!(harness.query_by_label("Edit").is_none());
    }
}
