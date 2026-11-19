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
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        {
            let mut response = output.response.clone();
            response.interact_rect = caret.expand(2.0).intersect(viewport);
            response.rect = response.interact_rect;
            crate::desktop_test::observe(&format!("overleaf.cursor.{}", cursor.id), &response);
        }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BadgeAction {
    Seek(String),
    Follow(String),
    StopFollowing,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Copy)]
struct PopupGeometry {
    frame: u64,
    viewport: egui::ViewportId,
    bounds: Rect,
}
#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl PopupGeometry {
    fn obscures(self, frame: u64, viewport: egui::ViewportId, rect: Rect) -> bool {
        self.frame == frame
            && self.viewport == viewport
            && self.bounds.is_finite()
            && rect.is_finite()
            && self.bounds.intersect(rect).is_positive()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn popup_geometry_id() -> egui::Id {
    egui::Id::new("overleaf-collaborator-popup-geometry")
}

/// Only the current menu frame can obscure a native preview. Closed menus and
/// data from another viewport cannot keep a child view hidden.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(super) fn popup_obscures(context: &egui::Context, rect: Rect) -> bool {
    context
        .data(|data| data.get_temp::<PopupGeometry>(popup_geometry_id()))
        .is_some_and(|popup| {
            popup.obscures(context.cumulative_frame_nr(), context.viewport_id(), rect)
        })
}

#[derive(Clone)]
struct PendingSeek {
    id: String,
    ready_at: f64,
}

#[derive(Clone)]
struct PeerClick {
    id: Arc<str>,
    count: u8,
    at: f64,
    position: Pos2,
}
impl PeerClick {
    fn next(
        previous: Option<&Self>,
        id: &str,
        at: f64,
        position: Pos2,
        delay: f64,
        distance: f32,
    ) -> Self {
        let count = previous
            .filter(|previous| {
                previous.id.as_ref() == id
                    && at >= previous.at
                    && at - previous.at < delay
                    && position.distance_sq(previous.position) < distance * distance
            })
            .map_or(1, |previous| previous.count % 3 + 1);
        Self {
            id: Arc::from(id),
            count,
            at,
            position,
        }
    }
}

fn badge_action(
    response: &egui::Response,
    id: &str,
    pending: &mut Option<PendingSeek>,
    clicks_id: egui::Id,
) -> Option<BadgeAction> {
    if !response.clicked() {
        return None;
    }
    // Keyboard and accessibility activation are a single seek, independent of
    // any mouse gesture already in progress.
    if !response.clicked_by(egui::PointerButton::Primary) {
        *pending = None;
        response
            .ctx
            .data_mut(|data| data.remove::<PeerClick>(clicks_id));
        return Some(BadgeAction::Seek(id.to_owned()));
    }
    let (delay, distance) = response.ctx.options(|options| {
        (
            options.input_options.max_double_click_delay,
            options.input_options.max_click_dist,
        )
    });
    let at = response.ctx.input(|input| input.time);
    let previous = response
        .ctx
        .data(|data| data.get_temp::<PeerClick>(clicks_id));
    // egui's global triple-click count can include opening the overflow menu or
    // clicking another peer. Only repeated clicks on this same peer count here.
    let click = PeerClick::next(
        previous.as_ref(),
        id,
        at,
        response
            .interact_pointer_pos()
            .unwrap_or(response.rect.center()),
        delay,
        distance,
    );
    let count = click.count;
    response
        .ctx
        .data_mut(|data| data.insert_temp(clicks_id, click));
    match count {
        3 => {
            *pending = None;
            Some(BadgeAction::StopFollowing)
        }
        2 => {
            *pending = None;
            Some(BadgeAction::Follow(id.to_owned()))
        }
        _ => {
            // Defer the single-click action so following never moves the local
            // caret first. There is only one pending seek for the whole row.
            *pending = Some(PendingSeek {
                id: id.to_owned(),
                ready_at: at + delay,
            });
            None
        }
    }
}

fn observe_badge(response: &egui::Response, index: usize, id: &str) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    {
        crate::desktop_test::observe(&format!("overleaf.collaborator.{index}"), response);
        crate::desktop_test::observe(&format!("overleaf.peer.{id}"), response);
    }
    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
    let _ = (response, index, id);
}

/// Header presence shares the existing title-bar row. Empty presence consumes
/// no layout space; overflow opens a compact list without adding another row.
pub(super) fn badges(
    ui: &mut Ui,
    collaborators: &[Collaborator],
    following: Option<&str>,
) -> Option<(Rect, Option<BadgeAction>)> {
    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    ui.ctx()
        .data_mut(|data| data.remove::<PopupGeometry>(popup_geometry_id()));
    let pending_id = ui.id().with("overleaf-pending-seek");
    let clicks_id = ui.id().with("overleaf-peer-clicks");
    let Some(layout) = BadgeLayout::new(ui.available_width(), collaborators.len()) else {
        ui.ctx().data_mut(|data| {
            data.remove::<PendingSeek>(pending_id);
            data.remove::<PeerClick>(clicks_id);
        });
        return None;
    };
    if ui.available_height() < BADGE_DIAMETER {
        ui.ctx().data_mut(|data| {
            data.remove::<PendingSeek>(pending_id);
            data.remove::<PeerClick>(clicks_id);
        });
        return None;
    }
    let (bounds, _) =
        ui.allocate_exact_size(Vec2::new(layout.width, BADGE_DIAMETER), Sense::hover());
    let painter = ui
        .painter()
        .with_clip_rect(bounds.intersect(ui.clip_rect()));
    let mut font = egui::TextStyle::Small.resolve(ui.style());
    font.size = font.size.min(10.0);
    let mut pending = ui
        .ctx()
        .data_mut(|data| data.get_temp::<PendingSeek>(pending_id));
    // A new interaction supersedes a delayed single click, including pressing
    // the same badge a second time; the release will then report double/triple.
    if pending.is_some()
        && ui.input(|input| {
            !input.focused
                || input.pointer.any_pressed()
                || input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { pressed: true, .. }
                            | egui::Event::Text(_)
                            | egui::Event::MouseWheel { .. }
                    )
                })
        })
    {
        pending = None;
    }
    let reset_clicks = ui.input(|input| {
        !input.focused
            || input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { pressed: true, .. }
                        | egui::Event::Text(_)
                        | egui::Event::MouseWheel { .. }
                )
            })
    });
    if reset_clicks {
        ui.ctx()
            .data_mut(|data| data.remove::<PeerClick>(clicks_id));
    }
    let mut peer_clicked = false;
    let mut action = None;
    for (index, collaborator) in collaborators.iter().take(layout.visible).enumerate() {
        let rect = Rect::from_min_size(
            bounds.min + Vec2::new(index as f32 * (BADGE_DIAMETER + BADGE_GAP), 0.0),
            Vec2::splat(BADGE_DIAMETER),
        );
        let selected = following == Some(collaborator.id.as_str());
        let response = ui
            .interact(
                rect,
                ui.id().with(("overleaf-collaborator", &collaborator.id)),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &collaborator.name)
        });
        observe_badge(&response, index, &collaborator.id);
        peer_clicked |= response.clicked();
        if let Some(clicked) = badge_action(&response, &collaborator.id, &mut pending, clicks_id) {
            action = Some(clicked);
        }
        let site_color = color(collaborator.color);
        painter.circle_filled(rect.center(), BADGE_DIAMETER * 0.5, site_color);
        if selected || response.hovered() || response.has_focus() {
            painter.circle_stroke(
                rect.center(),
                BADGE_DIAMETER * 0.5 - 1.0,
                Stroke::new(1.0, foreground(site_color)),
            );
        }
        if selected {
            painter.circle_stroke(
                rect.center(),
                BADGE_DIAMETER * 0.5 - 3.0,
                Stroke::new(1.0, foreground(site_color)),
            );
        }
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initials(&collaborator.name),
            font.clone(),
            foreground(site_color),
        );
    }
    if layout.overflow > 0 {
        let rect = Rect::from_min_size(
            bounds.min + Vec2::new(layout.visible as f32 * (BADGE_DIAMETER + BADGE_GAP), 0.0),
            Vec2::new(layout.overflow_width, BADGE_DIAMETER),
        );
        let selected = collaborators[layout.visible..]
            .iter()
            .any(|peer| following == Some(peer.id.as_str()));
        let response = ui
            .interact(
                rect,
                ui.id().with("overleaf-collaborators-overflow"),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                true,
                selected,
                format!("{} more collaborators", layout.overflow),
            )
        });
        painter.rect_filled(rect, 4.0, ui.visuals().widgets.inactive.bg_fill);
        if selected {
            for inset in [1.0, 3.0] {
                painter.rect_stroke(
                    rect.shrink(inset),
                    3.0,
                    Stroke::new(1.0, ui.visuals().text_color()),
                    egui::StrokeKind::Inside,
                );
            }
        }
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("+{}", layout.overflow),
            font,
            ui.visuals().text_color(),
        );
        let _popup = egui::Popup::menu(&response)
            .width(160.0)
            // Keep clicks open so double/triple clicks can start/stop following.
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_max_width(160.0);
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for (index, collaborator) in
                            collaborators.iter().enumerate().skip(layout.visible)
                        {
                            let response = ui
                                .push_id(("overleaf-collaborator-menu", &collaborator.id), |ui| {
                                    ui.add(
                                        egui::Button::new(&collaborator.name)
                                            .truncate()
                                            .selected(following == Some(collaborator.id.as_str())),
                                    )
                                })
                                .inner;
                            observe_badge(&response, index, &collaborator.id);
                            peer_clicked |= response.clicked();
                            if let Some(clicked) =
                                badge_action(&response, &collaborator.id, &mut pending, clicks_id)
                            {
                                action = Some(clicked);
                            }
                        }
                    });
            });
        #[cfg(any(target_os = "macos", target_os = "windows", test))]
        if let Some(popup) = _popup {
            let geometry = PopupGeometry {
                frame: ui.ctx().cumulative_frame_nr(),
                viewport: ui.ctx().viewport_id(),
                bounds: popup.response.rect,
            };
            ui.ctx()
                .data_mut(|data| data.insert_temp(popup_geometry_id(), geometry));
        }
    }
    if !peer_clicked && ui.input(|input| input.pointer.any_released()) {
        ui.ctx()
            .data_mut(|data| data.remove::<PeerClick>(clicks_id));
    }
    let previous = ui.ctx().data(|data| data.get_temp::<PeerClick>(clicks_id));
    if previous.is_some_and(|click| {
        !collaborators
            .iter()
            .any(|peer| peer.id == click.id.as_ref())
    }) {
        ui.ctx()
            .data_mut(|data| data.remove::<PeerClick>(clicks_id));
    }
    if let Some(seek) = &pending {
        if !collaborators.iter().any(|peer| peer.id == seek.id) {
            pending = None;
        } else {
            let remaining = seek.ready_at - ui.input(|input| input.time);
            if remaining <= 0.0 {
                action = Some(BadgeAction::Seek(seek.id.clone()));
                pending = None;
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(remaining));
            }
        }
    }
    ui.ctx().data_mut(|data| {
        if let Some(pending) = pending {
            data.insert_temp(pending_id, pending);
        } else {
            data.remove::<PendingSeek>(pending_id);
        }
    });
    Some((bounds, action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _},
    };

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
    fn collaborator_popup_only_obscures_intersecting_current_viewport_geometry() {
        let popup = PopupGeometry {
            frame: 7,
            viewport: egui::ViewportId::ROOT,
            bounds: Rect::from_min_max(Pos2::new(40.0, 20.0), Pos2::new(120.0, 100.0)),
        };
        let overlap = Rect::from_min_max(Pos2::new(80.0, 60.0), Pos2::new(200.0, 180.0));
        assert!(popup.obscures(7, egui::ViewportId::ROOT, overlap));
        assert!(!popup.obscures(8, egui::ViewportId::ROOT, overlap));
        assert!(!popup.obscures(7, egui::ViewportId::from_hash_of("other"), overlap));
        for outside in [
            Rect::from_min_max(Pos2::new(120.0, 20.0), Pos2::new(200.0, 100.0)),
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(20.0, 10.0)),
            Rect::NOTHING,
            Rect::from_min_size(Pos2::new(f32::NAN, 0.0), Vec2::splat(20.0)),
        ] {
            assert!(!popup.obscures(7, egui::ViewportId::ROOT, outside));
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
            assert!(badges(ui, &[], None).is_none());
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
                            let (bounds, _) = badges(ui, &people, None).unwrap();
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

    #[derive(Default)]
    struct BadgeState {
        actions: Vec<BadgeAction>,
        following: Option<String>,
        bounds: Option<Rect>,
        popup_obscures: bool,
    }

    fn badge_harness(width: f32) -> Harness<'static, BadgeState> {
        Harness::builder()
            .with_size(Vec2::new(320.0, 320.0))
            // Each queued pointer event runs a frame; the default 4 Hz makes
            // two clicks 750 ms apart, outside the actual multi-click window.
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                move |ui, state: &mut BadgeState| {
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(Rect::from_min_size(ui.cursor().min, Vec2::new(width, 24.0))),
                        |ui| {
                            let (bounds, action) =
                                badges(ui, &people(), state.following.as_deref()).unwrap();
                            state.bounds = Some(bounds);
                            state.popup_obscures = popup_obscures(
                                ui.ctx(),
                                Rect::from_min_size(Pos2::ZERO, Vec2::splat(320.0)),
                            );
                            if let Some(action) = action {
                                match &action {
                                    BadgeAction::Follow(id) => state.following = Some(id.clone()),
                                    BadgeAction::StopFollowing => state.following = None,
                                    BadgeAction::Seek(_) => {}
                                }
                                state.actions.push(action);
                            }
                        },
                    );
                },
                BadgeState::default(),
            )
    }

    #[test]
    fn badges_seek_after_the_multiclick_window_without_an_idle_repaint_loop() {
        let mut harness = badge_harness(120.0);
        harness.run();
        harness.get_by_label("Alice").click();
        harness.step();
        assert!(harness.state().actions.is_empty());
        let now = harness.ctx.input(|input| input.time);
        let delay = harness
            .ctx
            .options(|options| options.input_options.max_double_click_delay);
        harness.input_mut().time = Some(now + delay + 0.01);
        harness.run();
        assert_eq!(harness.state().actions, [BadgeAction::Seek("0".into())]);
        assert_ne!(
            harness.output().viewport_output[&egui::ViewportId::ROOT].repaint_delay,
            std::time::Duration::ZERO
        );
        harness.run();
        assert_eq!(harness.state().actions.len(), 1, "seek is dispatched once");
    }

    #[test]
    fn disappearing_collaborators_discard_pending_seek_without_replaying_on_reconnect() {
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut (Vec<Collaborator>, Vec<BadgeAction>)| {
                if let Some((_, Some(action))) = badges(ui, &state.0, None) {
                    state.1.push(action);
                }
            },
            (people(), Vec::new()),
        );
        harness.run();
        harness.get_by_label("Alice").click();
        harness.step();
        harness.state_mut().0.clear();
        harness.run();
        let now = harness.ctx.input(|input| input.time);
        harness.input_mut().time = Some(now + 1.0);
        harness.state_mut().0 = people();
        harness.run();
        assert!(harness.state().1.is_empty());
    }

    #[test]
    fn another_pointer_interaction_cancels_a_pending_seek() {
        let mut harness = badge_harness(120.0);
        harness.run();
        harness.get_by_label("Alice").click();
        harness.step();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: Pos2::new(300.0, 300.0),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.step();
        let now = harness.ctx.input(|input| input.time);
        harness.input_mut().time = Some(now + 1.0);
        harness.run();
        assert!(
            harness.state().actions.is_empty(),
            "a late seek must not undo a newer click"
        );
    }

    #[test]
    fn badges_follow_on_double_and_stop_on_triple_without_first_seeking() {
        let mut harness = badge_harness(120.0);
        harness.run();
        let bounds = harness.state().bounds;
        harness.get_by_label("Alice").click();
        harness.step();
        assert!(harness.state().actions.is_empty());
        harness.get_by_label("Alice").click();
        harness.step();
        assert_eq!(harness.state().actions, [BadgeAction::Follow("0".into())]);
        harness.run();
        assert_eq!(harness.state().following.as_deref(), Some("0"));
        assert_eq!(
            harness.state().bounds,
            bounds,
            "following cannot resize the header"
        );
        assert_eq!(
            harness.get_by_label("Alice").accesskit_node().toggled(),
            Some(egui::accesskit::Toggled::True)
        );
        harness.get_by_label("Alice").click();
        harness.step();
        assert_eq!(
            harness.state().actions.last(),
            Some(&BadgeAction::StopFollowing)
        );
        harness.run();
        assert_eq!(harness.state().following, None);
        assert_eq!(harness.state().bounds, bounds);
        assert_eq!(
            harness.get_by_label("Alice").accesskit_node().toggled(),
            Some(egui::accesskit::Toggled::False)
        );
        let now = harness.ctx.input(|input| input.time);
        harness.input_mut().time = Some(now + 1.0);
        harness.run();
        assert_eq!(
            harness.state().actions,
            [BadgeAction::Follow("0".into()), BadgeAction::StopFollowing],
            "double and triple clicks must cancel the pending single seek"
        );
        assert_ne!(
            harness.output().viewport_output[&egui::ViewportId::ROOT].repaint_delay,
            std::time::Duration::ZERO
        );
    }

    #[test]
    fn switching_peers_starts_a_new_badge_gesture() {
        let mut harness = badge_harness(120.0);
        harness.run();
        harness.get_by_label("Alice").click();
        harness.step();
        harness.get_by_label("Bob").click();
        harness.step();
        assert!(harness.state().actions.is_empty());
        harness.get_by_label("Bob").click();
        harness.step();
        assert_eq!(harness.state().actions, [BadgeAction::Follow("1".into())]);
        harness.get_by_label("Bob").click();
        harness.step();
        assert_eq!(
            harness.state().actions.last(),
            Some(&BadgeAction::StopFollowing)
        );
    }

    #[test]
    fn narrow_overflow_keeps_all_collaborators_actionable_without_resizing_the_header() {
        let mut harness = badge_harness(22.0);
        harness.run();
        let bounds = harness.state().bounds;
        assert!(!harness.state().popup_obscures);
        harness.get_by_label("7 more collaborators").click();
        harness.run();
        assert!(harness.state().popup_obscures);
        for person in people() {
            harness.get_by_label(&person.name);
        }
        harness.get_by_label("Frank").click();
        harness.step();
        assert!(harness.state().actions.is_empty());
        harness.get_by_label("Frank").click();
        harness.step();
        assert_eq!(
            harness.state().actions.last(),
            Some(&BadgeAction::Follow("5".into()))
        );
        harness.run();
        assert_eq!(harness.state().following.as_deref(), Some("5"));
        assert_eq!(
            harness
                .get_by_label("7 more collaborators")
                .accesskit_node()
                .toggled(),
            Some(egui::accesskit::Toggled::True)
        );
        assert_eq!(harness.state().bounds, bounds);
        assert_ne!(
            harness.output().viewport_output[&egui::ViewportId::ROOT].repaint_delay,
            std::time::Duration::ZERO
        );
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: Pos2::new(300.0, 300.0),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.run();
        assert!(
            !harness.state().popup_obscures,
            "closed popup geometry must expire immediately"
        );
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
