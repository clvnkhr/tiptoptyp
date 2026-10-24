//! Paint collaborator presence without changing editor layout or input ownership.
use crate::overleaf::{Collaborator, RemoteCursor};
use eframe::egui::{
    self, Color32, Galley, Pos2, Rect, Sense, Stroke, Ui, Vec2, epaint::text::cursor::LayoutCursor,
    text::CharIndex, text_edit::TextEditOutput,
};
use std::sync::{Arc, Weak};

const BADGE_DIAMETER: f32 = 18.0;
const BADGE_GAP: f32 = 3.0;
const BADGES_MAX_WIDTH: f32 = 120.0;
const FLAG_MAX_WIDTH: f32 = 120.0;

#[derive(Clone, Default)]
struct Geometry {
    galley: Weak<Galley>,
    row_starts: Vec<usize>,
    characters: usize,
    heads: Vec<usize>,
    carets: Arc<[Option<Rect>]>,
    #[cfg(test)]
    row_builds: usize,
    #[cfg(test)]
    mapped: usize,
}
impl Geometry {
    fn update(&mut self, galley: &Arc<Galley>, cursors: &[RemoteCursor]) -> Arc<[Option<Rect>]> {
        let same_galley = std::ptr::eq(self.galley.as_ptr(), Arc::as_ptr(galley));
        if same_galley
            && self.heads.len() == cursors.len()
            && self
                .heads
                .iter()
                .zip(cursors)
                .all(|(&head, cursor)| head == cursor.head)
        {
            return Arc::clone(&self.carets);
        }
        if !same_galley {
            self.galley = Arc::downgrade(galley);
            self.row_starts.clear();
            self.characters = 0;
            for row in &galley.rows {
                self.row_starts.push(self.characters);
                self.characters += row.char_count_including_newline().0;
            }
            #[cfg(test)]
            {
                self.row_builds += 1;
            }
        }
        self.heads.clear();
        self.heads.extend(cursors.iter().map(|cursor| cursor.head));
        self.carets = cursors
            .iter()
            .map(|cursor| self.locate(galley, cursor.head))
            .collect();
        #[cfg(test)]
        {
            self.mapped += cursors.len();
        }
        Arc::clone(&self.carets)
    }

    fn locate(&self, galley: &Galley, head: usize) -> Option<Rect> {
        if head > self.characters {
            return None;
        }
        // At a soft-wrap boundary use the following row, matching a caret
        // before that row's first character. Newlines retain their own row.
        let row = self
            .row_starts
            .partition_point(|&start| start <= head)
            .checked_sub(1)?;
        let placed = galley.rows.get(row)?;
        if placed.size.y <= 0.0 {
            return None;
        }
        let column = CharIndex(head - self.row_starts[row]);
        Some(galley.pos_from_layout_cursor(&LayoutCursor { row, column }))
    }
}

fn visible_caret(caret: Rect, viewport: Rect) -> Option<Rect> {
    if !caret.is_finite()
        || !viewport.is_finite()
        || !viewport.is_positive()
        || caret.height() <= 0.0
        || caret.left() < viewport.left()
        || caret.left() > viewport.right()
        || caret.bottom() <= viewport.top()
        || caret.top() >= viewport.bottom()
    {
        return None;
    }
    Some(Rect::from_min_max(
        Pos2::new(caret.left(), caret.top().max(viewport.top())),
        Pos2::new(caret.left(), caret.bottom().min(viewport.bottom())),
    ))
}

fn flag_bounds(caret: Rect, size: Vec2, viewport: Rect) -> Rect {
    let width = size.x.min(viewport.width()).max(0.0);
    let height = size.y.min(viewport.height()).max(0.0);
    let left = caret.left().clamp(
        viewport.left(),
        (viewport.right() - width).max(viewport.left()),
    );
    let top = (caret.top() - height).clamp(
        viewport.top(),
        (viewport.bottom() - height).max(viewport.top()),
    );
    Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height))
}

fn color(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}
fn foreground(background: Color32) -> Color32 {
    if u32::from(background.r()) * 299
        + u32::from(background.g()) * 587
        + u32::from(background.b()) * 114
        >= 145_000
    {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// Carets are paint-only: remote activity cannot reveal folds, move the local
/// selection, steal keyboard focus, or start a repaint loop.
pub(super) fn paint(ui: &Ui, output: &TextEditOutput, viewport: Rect, cursors: &[RemoteCursor]) {
    if cursors.is_empty() {
        return;
    }
    let viewport = viewport.intersect(ui.clip_rect());
    if !viewport.is_finite() || !viewport.is_positive() {
        return;
    }
    let carets = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Geometry>(output.response.id.with("overleaf-caret-geometry"))
            .update(&output.galley, cursors)
    });
    let painter = ui.painter().with_clip_rect(viewport);
    let mut font = egui::TextStyle::Small.resolve(ui.style());
    font.size = font.size.min(11.0);
    for (cursor, local) in cursors.iter().zip(carets.iter()) {
        let Some(caret) = local
            .and_then(|rect| visible_caret(rect.translate(output.galley_pos.to_vec2()), viewport))
        else {
            continue;
        };
        let site_color = color(cursor.color);
        painter.line_segment(
            [caret.left_top(), caret.left_bottom()],
            Stroke::new(2.0, site_color),
        );
        if cursor.name.is_empty() {
            continue;
        }
        let mut job = egui::text::LayoutJob::simple_singleline(
            cursor.name.clone(),
            font.clone(),
            foreground(site_color),
        );
        job.wrap.max_width = (FLAG_MAX_WIDTH.min(viewport.width()) - 6.0).max(1.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let name = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let flag = flag_bounds(caret, name.size() + Vec2::new(6.0, 2.0), viewport);
        painter.rect_filled(flag, 2.0, site_color);
        painter.with_clip_rect(flag.intersect(viewport)).galley(
            flag.min + Vec2::new(3.0, 1.0),
            name,
            foreground(site_color),
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BadgeLayout {
    visible: usize,
    overflow: usize,
    overflow_width: f32,
    width: f32,
}
impl BadgeLayout {
    fn new(available: f32, count: usize) -> Option<Self> {
        let available = available.min(BADGES_MAX_WIDTH);
        if count == 0 || !available.is_finite() || available < BADGE_DIAMETER {
            return None;
        }
        let slots = ((available + BADGE_GAP) / (BADGE_DIAMETER + BADGE_GAP)).floor() as usize;
        if count <= slots {
            return Some(Self {
                visible: count,
                overflow: 0,
                overflow_width: 0.0,
                width: count as f32 * (BADGE_DIAMETER + BADGE_GAP) - BADGE_GAP,
            });
        }
        let overflow_width = 24.0_f32.min(available);
        let visible =
            ((available - overflow_width) / (BADGE_DIAMETER + BADGE_GAP)).floor() as usize;
        Some(Self {
            visible,
            overflow: count - visible,
            overflow_width,
            width: visible as f32 * (BADGE_DIAMETER + BADGE_GAP) + overflow_width,
        })
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect()
}

/// Header presence shares the existing title-bar row. Empty presence consumes
/// no layout space; overflow is a compact count rather than another row.
pub(super) fn badges(ui: &mut Ui, collaborators: &[Collaborator]) -> Option<Rect> {
    let layout = BadgeLayout::new(ui.available_width(), collaborators.len())?;
    if ui.available_height() < BADGE_DIAMETER {
        return None;
    }
    let (bounds, _) =
        ui.allocate_exact_size(Vec2::new(layout.width, BADGE_DIAMETER), Sense::hover());
    let painter = ui
        .painter()
        .with_clip_rect(bounds.intersect(ui.clip_rect()));
    let mut font = egui::TextStyle::Small.resolve(ui.style());
    font.size = font.size.min(10.0);
    for (index, collaborator) in collaborators.iter().take(layout.visible).enumerate() {
        let rect = Rect::from_min_size(
            bounds.min + Vec2::new(index as f32 * (BADGE_DIAMETER + BADGE_GAP), 0.0),
            Vec2::splat(BADGE_DIAMETER),
        );
        let site_color = color(collaborator.color);
        painter.circle_filled(rect.center(), BADGE_DIAMETER * 0.5, site_color);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initials(&collaborator.name),
            font.clone(),
            foreground(site_color),
        );
        let response = ui.interact(
            rect,
            ui.id().with(("overleaf-collaborator", &collaborator.id)),
            Sense::hover(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &collaborator.name)
        });
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe(&format!("overleaf.collaborator.{index}"), &response);
    }
    if layout.overflow > 0 {
        let rect = Rect::from_min_size(
            bounds.min + Vec2::new(layout.visible as f32 * (BADGE_DIAMETER + BADGE_GAP), 0.0),
            Vec2::new(layout.overflow_width, BADGE_DIAMETER),
        );
        painter.rect_filled(rect, 4.0, ui.visuals().widgets.inactive.bg_fill);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("+{}", layout.overflow),
            font,
            ui.visuals().text_color(),
        );
        let response = ui.interact(
            rect,
            ui.id().with("overleaf-collaborators-overflow"),
            Sense::hover(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                true,
                format!("{} more collaborators", layout.overflow),
            )
        });
    }
    Some(bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    fn cursor(head: usize) -> RemoteCursor {
        RemoteCursor {
            id: "alice".into(),
            name: "Alice".into(),
            color: [17, 91, 203],
            anchor: head,
            head,
        }
    }
    fn people() -> Vec<Collaborator> {
        ["Alice", "Bob", "Chloé", "赵雷", "Eve", "Frank", "Grace"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| Collaborator {
                id: index.to_string(),
                name: name.into(),
                color: [17, 91, 203],
                doc_id: None,
            })
            .collect()
    }

    #[test]
    fn wrapped_unicode_carets_follow_glyph_rows_including_eof() {
        let context = egui::Context::default();
        context
            .run_ui(Default::default(), |ui| {
                let galley = ui.painter().layout(
                    "é😀 alpha beta gamma delta\n\nlast".into(),
                    egui::FontId::monospace(14.0),
                    Color32::WHITE,
                    80.0,
                );
                assert!(galley.rows.len() > 4, "fixture must soft-wrap");
                let cursors: Vec<_> = (0..=galley.text().chars().count()).map(cursor).collect();
                let mut geometry = Geometry::default();
                let carets = geometry.update(&galley, &cursors);
                for (cursor, actual) in cursors.iter().zip(carets.iter()) {
                    let mut at = egui::text::CCursor::new(cursor.head);
                    at.prefer_next_row = true;
                    assert_eq!(*actual, Some(galley.pos_from_cursor(at)), "{}", cursor.head);
                }
                assert_eq!(geometry.locate(&galley, usize::MAX), None);
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn final_folded_rows_have_no_remote_caret_and_visible_source_keeps_its_offset() {
        let context = egui::Context::default();
        let source = "#let f(x) = {\n  αβγ hidden inside the function\n}\nVisible Ω after fold";
        context
            .run_ui(Default::default(), |ui| {
                let mut folding = crate::folding::Folding::default();
                folding.prepare(
                    crate::document::DocumentKey::new(
                        tiptoptyp_core::document::WindowSessionId::new(1),
                        0,
                        0,
                    ),
                    Arc::from(source),
                    &crate::editor_features::context_regions(
                        &typst_syntax::Source::detached(source),
                        source,
                    ),
                );
                folding.toggle(0);
                let original = ui.painter().layout(
                    source.into(),
                    egui::FontId::monospace(14.0),
                    Color32::WHITE,
                    100.0,
                );
                let galley = folding.layout(original);
                let hidden = source[..source.find('α').unwrap()].chars().count();
                let visible = source[..source.find("Visible").unwrap()].chars().count();
                let carets =
                    Geometry::default().update(&galley, &[cursor(hidden), cursor(visible)]);
                assert_eq!(carets[0], None);
                assert_eq!(
                    carets[1],
                    Some(galley.pos_from_cursor(egui::text::CCursor::new(visible)))
                );
                assert!(
                    folding.is_collapsed(0),
                    "remote presence cannot expand the fold"
                );
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn geometry_cache_reuses_positions_on_idle_and_scroll_without_scanning_rows() {
        let context = egui::Context::default();
        context
            .run_ui(Default::default(), |ui| {
                let galley = ui.painter().layout(
                    "a long line that wraps\nsecond".into(),
                    egui::FontId::monospace(14.0),
                    Color32::WHITE,
                    70.0,
                );
                let mut cache = Geometry::default();
                let mut cursors = vec![cursor(3)];
                let first = cache.update(&galley, &cursors);
                cursors[0].name = "Renamed Alice".into();
                cursors[0].color = [255, 0, 0];
                let again = cache.update(&galley, &cursors);
                assert!(Arc::ptr_eq(&first, &again));
                assert_eq!((cache.row_builds, cache.mapped), (1, 1));
                let offset = Vec2::new(-80.0, -100.0);
                assert_eq!(
                    first[0].unwrap().translate(offset),
                    again[0].unwrap().translate(offset)
                );
                cursors[0].head = 4;
                let _ = cache.update(&galley, &cursors);
                assert_eq!((cache.row_builds, cache.mapped), (1, 2));
                let changed = Arc::new((*galley).clone());
                let _ = cache.update(&changed, &cursors);
                assert_eq!((cache.row_builds, cache.mapped), (2, 3));
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn carets_and_name_flags_stay_inside_the_visible_text_area() {
        let viewport = Rect::from_min_size(Pos2::new(40.0, 50.0), Vec2::new(80.0, 40.0));
        for x in [viewport.left(), viewport.right()] {
            let caret = Rect::from_min_max(Pos2::new(x, 45.0), Pos2::new(x, 70.0));
            let visible = visible_caret(caret, viewport).unwrap();
            assert_eq!(visible.top(), viewport.top());
            assert!(
                viewport.contains(visible.left_top()) && viewport.contains(visible.left_bottom())
            );
            for size in [Vec2::new(200.0, 14.0), Vec2::splat(500.0)] {
                assert!(viewport.contains_rect(flag_bounds(visible, size, viewport)));
            }
        }
        for caret in [
            Rect::from_min_max(Pos2::new(39.0, 50.0), Pos2::new(39.0, 70.0)),
            Rect::from_min_max(Pos2::new(50.0, 90.0), Pos2::new(50.0, 110.0)),
            Rect::from_min_max(Pos2::new(50.0, 40.0), Pos2::new(50.0, 50.0)),
        ] {
            assert!(visible_caret(caret, viewport).is_none());
        }
    }

    #[test]
    fn badge_layout_is_bounded_and_empty_presence_uses_no_space() {
        for available in [0.0, 10.0, 18.0, 22.0, 40.0, 100.0, 120.0, 500.0] {
            assert!(BadgeLayout::new(available, 0).is_none());
            for count in [1, 2, 7, 100] {
                if let Some(layout) = BadgeLayout::new(available, count) {
                    assert!(layout.width <= available.min(BADGES_MAX_WIDTH));
                    assert_eq!(layout.visible + layout.overflow, count);
                }
            }
        }
        let mut harness = Harness::new_ui(|ui| {
            let before = ui.cursor();
            assert!(badges(ui, &[]).is_none());
            assert_eq!(ui.cursor(), before);
        });
        harness.run();
    }

    #[test]
    fn compact_badges_keep_full_accessible_names_and_narrow_overflow() {
        for (width, visible, overflow) in [(120.0, 4, 3), (22.0, 0, 7)] {
            let people = people();
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width + 40.0, 80.0))
                .build_ui(move |ui| {
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(Rect::from_min_size(ui.cursor().min, Vec2::new(width, 24.0))),
                        |ui| {
                            let bounds = badges(ui, &people).unwrap();
                            assert!(bounds.width() <= width);
                            assert_eq!(bounds.height(), BADGE_DIAMETER);
                        },
                    );
                });
            harness.run();
            if visible > 0 {
                harness.get_by_label("Alice");
            } else {
                assert!(harness.query_by_label("Alice").is_none());
            }
            let overflow_label = format!("{overflow} more collaborators");
            harness.get_by_label(&overflow_label);
        }
    }

    #[test]
    fn painting_presence_preserves_local_selection_focus_and_idle_repaint() {
        let mut source = "alpha beta gamma".to_owned();
        let mut harness = Harness::new_ui(|ui| {
            let output = egui::TextEdit::multiline(&mut source)
                .desired_width(300.0)
                .show(ui);
            let focus = ui.memory(|memory| memory.focused());
            let selection = output.state.cursor.char_range();
            paint(ui, &output, output.response.rect, &[cursor(6)]);
            assert_eq!(ui.memory(|memory| memory.focused()), focus);
            assert_eq!(output.state.cursor.char_range(), selection);
        });
        harness.run();
        assert_ne!(
            harness.output().viewport_output[&egui::ViewportId::ROOT].repaint_delay,
            std::time::Duration::ZERO
        );
    }
}
