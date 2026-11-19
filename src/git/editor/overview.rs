//! A click-through overview using the source surface's real scrollbar geometry.
use super::{ChangeKind, Hunk};
use eframe::egui::{self, Rangef, Rect, pos2};

const WIDTH: f32 = 4.0;
const INSET: f32 = 1.0;
const MIN_HEIGHT: f32 = 2.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Marker {
    pub(super) kind: ChangeKind,
    pub(super) content: Rangef,
}
impl Marker {
    pub(crate) fn new(kind: ChangeKind, content: Rangef) -> Self {
        Self { kind, content }
    }
}

/// Positions belong to one rendered content snapshot. The origin accounts for
/// TextEdit's margin; it is independent of the current scroll offset.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geometry {
    pub(crate) content_height: f32,
    pub(crate) offset: f32,
    pub(crate) content_origin: f32,
}

/// Resolve only changed ranges through the existing logical-to-rendered row
/// index. Wrapping, diagnostic rows and collapsed lines are already in it.
pub(crate) fn markers(
    hunks: &[Hunk],
    lines: usize,
    mut row: impl FnMut(usize) -> Option<Rangef>,
) -> Vec<Marker> {
    let Some(last) = lines.checked_sub(1) else {
        return Vec::new();
    };
    hunks
        .iter()
        .flat_map(|hunk| &hunk.changes)
        .filter_map(|change| {
            let first = row(change.lines.start.min(last))?;
            let content = if change.lines.is_empty() {
                // The gutter paints a three-point deletion at this boundary.
                let anchor = if change.lines.start >= lines {
                    first.max
                } else {
                    first.min
                };
                Rangef::new((anchor - 1.5).max(0.0), anchor + 1.5)
            } else {
                let end = row(change.lines.end.saturating_sub(1).min(last))?;
                Rangef::new(first.min, end.max)
            };
            Some(Marker::new(change.kind, content))
        })
        .collect()
}

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

/// Match egui ScrollArea's handle_size and handle_start_point calculations.
/// A minimum-sized thumb changes its travel, so content-height percentages
/// alone are insufficient even when every logical line is equally tall.
#[derive(Clone, Copy, Debug)]
struct Mapping {
    track: Rect,
    content_height: f32,
    viewport_height: f32,
    max_offset: f32,
    thumb_height: f32,
}
impl Mapping {
    fn new(track: Rect, content_height: f32, viewport_height: f32, minimum: f32) -> Option<Self> {
        if !content_height.is_finite()
            || !viewport_height.is_finite()
            || !minimum.is_finite()
            || content_height < 0.0
            || viewport_height <= 0.0
        {
            return None;
        }
        let thumb_height = if content_height > viewport_height {
            (track.height() * viewport_height / content_height)
                .max(minimum)
                .clamp(0.0, track.height())
        } else {
            track.height()
        };
        Some(Self {
            track,
            content_height,
            viewport_height,
            max_offset: (content_height - viewport_height).max(0.0),
            thumb_height,
        })
    }
    fn position(self, content_y: f32) -> f32 {
        if self.max_offset == 0.0 {
            // There is no scrollbar; spread changes through the short source.
            self.track.top()
                + self.track.height() * (content_y / self.content_height.max(1.0)).clamp(0.0, 1.0)
        } else {
            self.track.top()
                + (self.track.height() - self.thumb_height)
                    * (content_y / self.max_offset).clamp(0.0, 1.0)
        }
    }
    fn thumb(self, offset: f32) -> Rangef {
        let top = if self.max_offset == 0.0 {
            self.track.top()
        } else {
            self.position(offset)
        };
        Rangef::new(top, top + self.thumb_height)
    }
    fn marker(self, content: Rangef) -> Option<Rect> {
        if !content.min.is_finite() || !content.max.is_finite() || content.max < content.min {
            return None;
        }
        let start = content.min.clamp(0.0, self.content_height);
        let end = content.max.clamp(start, self.content_height);
        let bottom = self.position((end - 0.5).max(start));
        let natural_top = self.position(start);
        // Grow thin marks upward only within scroll positions where the hunk
        // intersects the viewport. Never promise visibility by painting beyond
        // the range a tiny/minimum thumb can represent. Very large documents
        // can require subpixel marks to preserve that invariant.
        let earliest_visible = if self.max_offset == 0.0 {
            self.track.top()
        } else {
            self.position((start - self.viewport_height + 0.5).max(0.0))
        };
        let top = natural_top.min(bottom - MIN_HEIGHT).max(earliest_visible);
        let rect = Rect::from_min_max(
            pos2(self.track.left(), top),
            pos2(self.track.right(), bottom),
        );
        rect.is_positive().then_some(rect)
    }
}

/// Merge overlapping ticks per color without allocating a second source index.
fn visit_markers(
    markers: &[Marker],
    mapping: Mapping,
    origin: f32,
    mut visit: impl FnMut(ChangeKind, Rect),
) {
    for kind in [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted] {
        let mut pending: Option<Rect> = None;
        for marker in markers.iter().filter(|marker| marker.kind == kind) {
            let content = Rangef::new(marker.content.min + origin, marker.content.max + origin);
            let Some(rect) = mapping.marker(content) else {
                continue;
            };
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

pub(crate) fn paint(ui: &egui::Ui, viewport: Rect, geometry: Geometry, markers: &[Marker]) {
    let Some(track) = track(viewport) else {
        return;
    };
    let minimum_thumb_height = ui.spacing().scroll.handle_min_length;
    let Some(mapping) = Mapping::new(
        track,
        geometry.content_height,
        viewport.height(),
        minimum_thumb_height,
    ) else {
        return;
    };
    let painter = ui
        .painter()
        .with_clip_rect(viewport.intersect(ui.clip_rect()));
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    let mut ticks = Vec::new();
    let palette = crate::theme::palette(ui.ctx());
    let _thumb = mapping.thumb(geometry.offset);
    visit_markers(markers, mapping, geometry.content_origin, |kind, rect| {
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
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                id,
                Observation {
                    frame,
                    viewport,
                    track,
                    geometry,
                    thumb: _thumb,
                    minimum_thumb_height,
                    ticks,
                    markers: markers.to_vec(),
                },
            )
        });
    }
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
#[derive(Clone)]
struct Observation {
    frame: u64,
    viewport: Rect,
    track: Rect,
    geometry: Geometry,
    thumb: Rangef,
    minimum_thumb_height: f32,
    ticks: Vec<(ChangeKind, Rect)>,
    markers: Vec<Marker>,
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
pub(crate) fn inspection(context: &egui::Context) -> Option<serde_json::Value> {
    let id = context.viewport_id().0.with("git-overview-observation");
    let Observation {
        frame,
        viewport,
        track,
        geometry,
        thumb,
        minimum_thumb_height,
        ticks,
        markers,
    } = context.data(|data| data.get_temp::<Observation>(id))?;
    (frame == context.cumulative_frame_nr()).then(|| {
        let rect = |r: Rect| [r.left(), r.top(), r.right(), r.bottom()];
        serde_json::json!({"viewport":rect(viewport), "track":rect(track),
            "content_height":geometry.content_height, "offset":geometry.offset,
            "thumb":[thumb.min,thumb.max], "minimum_thumb_height":minimum_thumb_height,
            "rendered_changes":markers.into_iter().map(|marker| serde_json::json!({
                "kind":format!("{:?}",marker.kind),
                "content":[marker.content.min+geometry.content_origin,marker.content.max+geometry.content_origin],
            })).collect::<Vec<_>>(), "ticks":ticks.into_iter()
            .map(|(kind,r)| serde_json::json!({"kind":format!("{kind:?}"),"rect":rect(r)})).collect::<Vec<_>>()})
    })
}

#[cfg(test)]
mod tests {
    use super::super::LineChange;
    use super::*;
    fn viewport() -> Rect {
        Rect::from_min_max(pos2(20.0, 40.0), pos2(420.0, 640.0))
    }
    fn mapping(content_height: f32, minimum: f32) -> Mapping {
        Mapping::new(track(viewport()).unwrap(), content_height, 600.0, minimum).unwrap()
    }
    fn offset_at(mapping: Mapping, head: f32) -> f32 {
        (head - mapping.track.top()) / (mapping.track.height() - mapping.thumb_height)
            * mapping.max_offset
    }
    fn assert_visible_at_every_contact(mapping: Mapping, content: Rangef) {
        let mark = mapping.marker(content).expect("visible tick");
        for head in [mark.top(), mark.center().y, mark.bottom() - 0.001] {
            let offset = offset_at(mapping, head);
            assert!(
                offset < content.max && offset + mapping.viewport_height > content.min,
                "head={head} offset={offset} content={content:?} mark={mark:?}"
            );
        }
    }

    #[test]
    fn thumb_contact_reveals_wrapped_diagnostic_and_folded_rendered_ranges() {
        let mapping = mapping(9000.0, 12.0);
        // The first hunk is logical line five, but a wrapped first line plus
        // diagnostic rows puts it thousands of points into the rendered source.
        let content = Rangef::new(5300.0, 5340.0);
        let old_tick = mapping.track.top() + mapping.track.height() * 5.0 / 181.0;
        assert!(
            offset_at(mapping, old_tick) + 600.0 < content.min,
            "regression fixture must fail with the old logical-line mapping"
        );
        assert_visible_at_every_contact(mapping, content);
        assert_visible_at_every_contact(mapping, Rangef::new(15.0, 31.0));
        assert_visible_at_every_contact(mapping, Rangef::new(8970.0, 9000.0));
    }

    #[test]
    fn minimum_thumb_length_and_subpixel_marks_preserve_contact_visibility() {
        let mapping = mapping(1_000_000.0, 32.0);
        assert_eq!(mapping.thumb_height, 32.0);
        let content = Rangef::new(530_000.0, 530_020.0);
        let marker = mapping.marker(content).unwrap();
        assert!(marker.height() < MIN_HEIGHT);
        assert_visible_at_every_contact(mapping, content);
        for offset in [0.0, 530_000.0, mapping.max_offset] {
            assert!((offset_at(mapping, mapping.thumb(offset).min) - offset).abs() < 0.1);
        }
    }

    #[test]
    fn changes_resolve_rendered_rows_and_eof_deletions_without_a_source_scan() {
        let hunks = [Hunk {
            text: String::new(),
            changes: vec![
                LineChange {
                    kind: ChangeKind::Modified,
                    lines: 1..3,
                    old_line_count: 2,
                    new_line_count: 2,
                },
                LineChange {
                    kind: ChangeKind::Deleted,
                    lines: 3..3,
                    old_line_count: 1,
                    new_line_count: 0,
                },
            ],
        }];
        let rows = [
            Rangef::new(0.0, 1000.0),
            Rangef::new(1000.0, 1020.0),
            Rangef::new(1020.0, 1050.0),
        ];
        let mut lookups = 0;
        let marks = markers(&hunks, rows.len(), |line| {
            lookups += 1;
            rows.get(line).copied()
        });
        assert_eq!(lookups, 3);
        assert_eq!(marks[0].content, Rangef::new(1000.0, 1050.0));
        assert_eq!(marks[1].content, Rangef::new(1048.5, 1051.5));
        assert_visible_at_every_contact(mapping(1050.0, 12.0), marks[1].content);
    }

    #[test]
    fn short_empty_and_invalid_geometry_remains_inside_the_viewport() {
        for height in [0.0, 1.0, 300.0, 600.0] {
            let mapping = mapping(height, 12.0);
            if let Some(rect) = mapping.marker(Rangef::new(0.0, height)) {
                assert!(viewport().contains_rect(rect));
            }
        }
        assert!(track(Rect::NOTHING).is_none());
        assert!(track(Rect::from_min_max(pos2(0.0, 0.0), pos2(0.0, 30.0))).is_none());
        assert!(Mapping::new(viewport(), f32::NAN, 600.0, 12.0).is_none());
        assert!(
            mapping(1000.0, 12.0)
                .marker(Rangef::new(5.0, 3.0))
                .is_none()
        );
    }

    #[test]
    fn dense_changes_merge_without_merging_colors() {
        let mapping = mapping(600_000.0, 12.0);
        let marks = (0..30_000)
            .map(|i| {
                Marker::new(
                    [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted][i % 3],
                    Rangef::new(i as f32 * 20.0, (i + 1) as f32 * 20.0),
                )
            })
            .collect::<Vec<_>>();
        let mut ticks = Vec::new();
        visit_markers(&marks, mapping, 0.0, |k, r| ticks.push((k, r)));
        assert!(ticks.len() <= 3 * (viewport().height() / MIN_HEIGHT).ceil() as usize);
        for kind in [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted] {
            assert!(ticks.iter().any(|(k, _)| *k == kind));
        }
    }

    #[test]
    fn painting_does_not_allocate_space() {
        use egui_kittest::Harness;
        let mut harness = Harness::new_ui(|ui| {
            let before = (ui.min_rect(), ui.available_rect_before_wrap());
            let marks = [Marker::new(
                ChangeKind::Modified,
                Rangef::new(1000.0, 1020.0),
            )];
            paint(
                ui,
                before.1,
                Geometry {
                    content_height: 2000.0,
                    offset: 0.0,
                    content_origin: 0.0,
                },
                &marks,
            );
            assert_eq!((ui.min_rect(), ui.available_rect_before_wrap()), before);
        });
        harness.run();
    }
}
