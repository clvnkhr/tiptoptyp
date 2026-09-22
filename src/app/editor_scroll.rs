//! Keep a source character at its screen position while wrapped text reflows.
use std::sync::Arc;

use eframe::egui::{self, Pos2, Rect, Vec2, text::CCursor};

use crate::document::DocumentKey;

#[derive(Clone)]
pub(super) struct Snapshot {
    pub document: DocumentKey,
    pub available_size: Vec2,
    pub viewport: Rect,
    pub galley: Arc<egui::Galley>,
    pub galley_pos: Pos2,
}

impl Snapshot {
    pub fn resize_anchor(
        &self,
        document: DocumentKey,
        available_size: Vec2,
        caret: CCursor,
    ) -> Option<Anchor> {
        if self.document != document || self.available_size == available_size {
            return None;
        }
        let caret_rect = self
            .galley
            .pos_from_cursor(caret)
            .translate(self.galley_pos.to_vec2());
        let caret_visible = caret_rect.bottom() >= self.viewport.top()
            && caret_rect.top() <= self.viewport.bottom();
        let cursor = if caret_visible {
            caret
        } else {
            let center = self
                .galley
                .cursor_from_pos(self.viewport.center() - self.galley_pos);
            self.galley.cursor_begin_of_paragraph(&center)
        };
        Some(Anchor {
            cursor,
            screen_y: self.galley.pos_from_cursor(cursor).center().y + self.galley_pos.y,
            follows_center: !caret_visible,
            previous_center_y: self.viewport.center().y,
        })
    }
}

pub(super) struct Anchor {
    cursor: CCursor,
    screen_y: f32,
    follows_center: bool,
    previous_center_y: f32,
}

impl Anchor {
    pub fn scroll_delta(&self, galley: &egui::Galley, galley_pos: Pos2, viewport: Rect) -> f32 {
        let destination = if self.follows_center {
            self.screen_y + viewport.center().y - self.previous_center_y
        } else {
            self.screen_y.clamp(viewport.top(), viewport.bottom())
        };
        destination - (galley.pos_from_cursor(self.cursor).center().y + galley_pos.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "optimized, opt-in source resize preparation measurement"]
    fn measure_resize_anchor_preparation() {
        use std::{hint::black_box, time::Instant};
        let context = egui::Context::default();
        let text = "A wrapped source line with Unicode αβ and enough text for a second row.\n"
            .repeat(10_000);
        let mut galley = None;
        context
            .run_ui(Default::default(), |ui| {
                galley = Some(ui.painter().layout(
                    text.clone(),
                    egui::FontId::monospace(13.0),
                    egui::Color32::BLACK,
                    500.0,
                ));
            })
            .drop_without_applying_deltas();
        let snapshot = Snapshot {
            document: DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 0, 0),
            available_size: Vec2::new(520.0, 600.0),
            viewport: Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 600.0)),
            galley: galley.unwrap(),
            galley_pos: Pos2::new(0.0, -80_000.0),
        };
        let caret = snapshot
            .galley
            .cursor_from_pos(snapshot.viewport.center() - snapshot.galley_pos);
        for (name, size) in [
            ("idle", snapshot.available_size),
            ("resize", Vec2::new(400.0, 600.0)),
        ] {
            for _ in 0..100 {
                black_box(snapshot.resize_anchor(snapshot.document, size, caret));
            }
            let start = Instant::now();
            for _ in 0..10_000 {
                black_box(snapshot.clone());
            }
            let baseline = start.elapsed();
            let start = Instant::now();
            for _ in 0..10_000 {
                let retained = black_box(snapshot.clone());
                if let Some(anchor) = retained.resize_anchor(snapshot.document, size, caret) {
                    black_box(anchor.scroll_delta(
                        &snapshot.galley,
                        snapshot.galley_pos,
                        snapshot.viewport,
                    ));
                }
            }
            eprintln!(
                "{name}: 10000 iterations, clone baseline={baseline:?}, anchor preparation={:?}, rows={}",
                start.elapsed(),
                snapshot.galley.rows.len()
            );
        }
    }
}
