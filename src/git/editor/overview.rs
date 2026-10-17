//! A click-through, width-free overview of changes across the whole document.
use super::{ChangeKind, Hunk, LineChange};
use eframe::egui::{self, Rect, pos2};

const WIDTH: f32 = 4.0;
const INSET: f32 = 1.0;
const MIN_HEIGHT: f32 = 2.0;

fn track(viewport: Rect) -> Option<Rect> {
    (viewport.is_finite() && viewport.width() > INSET && viewport.height() > 0.0).then(|| {
        Rect::from_min_max(
            pos2(
                (viewport.right() - INSET - WIDTH).max(viewport.left()),
                viewport.top(),
            ),
            pos2(viewport.right() - INSET, viewport.bottom()),
        )
    })
}

fn marker(track: Rect, lines: usize, change: &LineChange) -> Rect {
    let lines = lines.max(1);
    let y =
        |line: usize| track.top() + track.height() * (line.min(lines) as f64 / lines as f64) as f32;
    let top = y(change.lines.start);
    let bottom = y(change.lines.end.max(change.lines.start));
    let height = (bottom - top).max(MIN_HEIGHT).min(track.height());
    let top = top.min(track.bottom() - height);
    Rect::from_min_max(pos2(track.left(), top), pos2(track.right(), top + height))
}

/// Hunk ranges are ordered by their current-document line. Merge overlapping
/// ticks per color so dense changes emit at most a viewport's worth of marks.
/// No source scan, scratch allocation, widget, or repaint is needed.
fn visit_markers(
    hunks: &[Hunk],
    lines: usize,
    track: Rect,
    mut visit: impl FnMut(ChangeKind, Rect),
) {
    for kind in [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted] {
        let mut pending: Option<Rect> = None;
        for change in hunks
            .iter()
            .flat_map(|hunk| &hunk.changes)
            .filter(|change| change.kind == kind)
        {
            let rect = marker(track, lines, change);
            if let Some(last) = pending.as_mut()
                && rect.top() <= last.bottom()
            {
                *last = last.union(rect);
            } else {
                if let Some(last) = pending {
                    visit(kind, last);
                }
                pending = Some(rect);
            }
        }
        if let Some(last) = pending {
            visit(kind, last);
        }
    }
}

pub(crate) fn paint(ui: &egui::Ui, viewport: Rect, lines: usize, hunks: &[Hunk]) {
    let Some(track) = track(viewport) else {
        return;
    };
    let painter = ui
        .painter()
        .with_clip_rect(viewport.intersect(ui.clip_rect()));
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    let mut ticks = Vec::new();
    let palette = crate::theme::palette(ui.ctx());
    visit_markers(hunks, lines, track, |kind, rect| {
        let color = match kind {
            ChangeKind::Added => palette.success,
            ChangeKind::Modified => palette.info,
            ChangeKind::Deleted => palette.error,
        };
        painter.rect_filled(rect, 0.0, color);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        ticks.push((kind, rect));
    });
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    {
        let id = ui.ctx().viewport_id().0.with("git-overview-observation");
        let frame = ui.ctx().cumulative_frame_nr();
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, (frame, viewport, track, ticks)));
    }
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
pub(crate) fn inspection(context: &egui::Context) -> Option<serde_json::Value> {
    type Observation = (u64, Rect, Rect, Vec<(ChangeKind, Rect)>);
    let id = context.viewport_id().0.with("git-overview-observation");
    let (frame, viewport, track, ticks) = context.data(|data| data.get_temp::<Observation>(id))?;
    (frame == context.cumulative_frame_nr()).then(|| {
        let rect = |r: Rect| [r.left(), r.top(), r.right(), r.bottom()];
        serde_json::json!({"viewport":rect(viewport), "track":rect(track), "ticks":ticks.into_iter()
            .map(|(kind,r)| serde_json::json!({"kind":format!("{kind:?}"),"rect":rect(r)})).collect::<Vec<_>>()})
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn change(kind: ChangeKind, lines: std::ops::Range<usize>) -> LineChange {
        LineChange {
            old_line_count: lines.len(),
            new_line_count: lines.len(),
            kind,
            lines,
        }
    }
    fn ticks(hunks: &[Hunk], lines: usize, viewport: Rect) -> Vec<(ChangeKind, Rect)> {
        let mut ticks = Vec::new();
        if let Some(track) = track(viewport) {
            visit_markers(hunks, lines, track, |k, r| ticks.push((k, r)));
        }
        ticks
    }
    fn viewport() -> Rect {
        Rect::from_min_max(pos2(20.0, 40.0), pos2(420.0, 640.0))
    }

    #[test]
    fn global_positions_cover_offscreen_changes_and_preserve_deletions_at_eof() {
        let viewport = viewport();
        let hunks = [Hunk {
            text: String::new(),
            changes: vec![
                change(ChangeKind::Added, 0..10),
                change(ChangeKind::Modified, 500..510),
                change(ChangeKind::Deleted, 1000..1000),
            ],
        }];
        let ticks = ticks(&hunks, 1000, viewport);
        assert_eq!(ticks.len(), 3);
        assert_eq!(ticks[0].1.top(), 40.0);
        assert_eq!(ticks[1].1.top(), 340.0);
        assert_eq!(ticks[2].1.bottom(), 640.0);
        assert_eq!(ticks[2].1.height(), MIN_HEIGHT);
        for (_, r) in ticks {
            assert!(viewport.contains_rect(r));
            assert_eq!(r.width(), WIDTH);
        }
    }

    #[test]
    fn empty_short_and_stale_line_ranges_stay_within_the_existing_viewport() {
        let hunks = [Hunk {
            text: String::new(),
            changes: vec![change(ChangeKind::Deleted, 99..99)],
        }];
        for lines in [0, 1, 10] {
            let t = ticks(&hunks, lines, viewport());
            assert!(viewport().contains_rect(t[0].1));
        }
        let narrow = Rect::from_min_max(pos2(0.0, 0.0), pos2(3.0, 1.0));
        assert!(narrow.contains_rect(ticks(&hunks, 1, narrow)[0].1));
        assert!(track(Rect::NOTHING).is_none());
        assert!(track(Rect::from_min_max(pos2(0.0, 0.0), pos2(0.0, 30.0))).is_none());
        assert!(ticks(&[], 100, viewport()).is_empty());
    }

    #[test]
    fn dense_changes_have_bounded_paint_work_without_merging_colors() {
        let hunks = [Hunk {
            text: String::new(),
            changes: (0..30_000)
                .map(|i| {
                    change(
                        [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted][i % 3],
                        i..i + 1,
                    )
                })
                .collect(),
        }];
        let ticks = ticks(&hunks, 30_000, viewport());
        assert!(ticks.len() <= 3 * (viewport().height() / MIN_HEIGHT).ceil() as usize);
        assert!(ticks.iter().any(|(kind, _)| *kind == ChangeKind::Added));
        assert!(ticks.iter().any(|(kind, _)| *kind == ChangeKind::Modified));
        assert!(ticks.iter().any(|(kind, _)| *kind == ChangeKind::Deleted));
    }

    #[test]
    fn painting_does_not_allocate_space() {
        use egui_kittest::Harness;
        let mut harness = Harness::new_ui(|ui| {
            let before = (ui.min_rect(), ui.available_rect_before_wrap());
            let track = track(before.1).unwrap();
            let hunks = [Hunk {
                text: String::new(),
                changes: vec![change(ChangeKind::Modified, 1..2)],
            }];
            paint(ui, before.1, 20, &hunks);
            assert_eq!((ui.min_rect(), ui.available_rect_before_wrap()), before);
            assert!(before.1.contains_rect(track));
        });
        harness.run();
    }
}
