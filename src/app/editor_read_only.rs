//! Finite, source-owned rejection feedback; no text edits, layout changes or workers.
use super::*;

pub(super) const MESSAGE: &str = "Read-only · edit not applied";
const LIFETIME: f64 = 1.6;
const PULSE_SECONDS: f64 = 0.35;
const FADE_SECONDS: f64 = 0.2;
const FRAME_SECONDS: f64 = 1.0 / 60.0;
const MARGIN: f32 = 6.0;
const PADDING: Vec2 = Vec2::new(8.0, 4.0);

#[derive(Clone, Copy)]
struct Rejection {
    document: DocumentKey,
    born: f64,
    painted_frame: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    opacity: f32,
    pulse: f32,
    wake_after: f64,
}

fn sample(born: f64, now: f64) -> Option<Frame> {
    let age = now - born;
    if !age.is_finite() || now < born || now >= born + LIFETIME {
        return None;
    }
    let fade_at = LIFETIME - FADE_SECONDS;
    let wake_after = if age < PULSE_SECONDS {
        FRAME_SECONDS.min(PULSE_SECONDS - age)
    } else if age < fade_at {
        fade_at - age
    } else {
        FRAME_SECONDS.min(LIFETIME - age)
    };
    Some(Frame {
        opacity: ((LIFETIME - age) / FADE_SECONDS).clamp(0.0, 1.0) as f32,
        pulse: (1.0 - age / PULSE_SECONDS).clamp(0.0, 1.0) as f32,
        wake_after,
    })
}

fn state_id(editor: egui::Id) -> egui::Id {
    editor.with("read-only-rejection")
}

pub(super) fn reject(context: &egui::Context, editor: egui::Id, document: DocumentKey) {
    let born = context.input(|input| input.time);
    // Repeated keys replace a single pulse instead of accumulating effects.
    context.data_mut(|data| {
        data.insert_temp(
            state_id(editor),
            Rejection {
                document,
                born,
                painted_frame: None,
            },
        );
    });
    context.request_repaint();
}

impl EditorApp {
    pub(super) fn source_edit_blocked(&self) -> bool {
        self.tabs.is_empty() || !self.document().kind().is_editable() || self.source_edit_locked()
    }

    pub(super) fn source_edit_locked(&self) -> bool {
        self.overleaf_source_locked()
            || self.table_editor.is_some()
            || self.tool_dialog_open()
            || self.liquid.visible()
            || self.diagnostic_fix.visible()
            || self.version_history.is_some()
            || self.overleaf.review.is_some()
    }

    pub(super) fn reject_read_only_edit(&self, context: &egui::Context) -> bool {
        if !self.source_edit_blocked() {
            return false;
        }
        reject(context, source_editor_id(context), self.document().key());
        true
    }
}

fn badge_rect(viewport: Rect, text_size: Vec2) -> Option<Rect> {
    if !viewport.is_finite() || !viewport.is_positive() || !text_size.is_finite() {
        return None;
    }
    let margin = Vec2::splat(MARGIN).min(viewport.size() * 0.25);
    let available = viewport.size() - margin * 2.0;
    let size = (text_size + PADDING * 2.0).min(available);
    Some(Rect::from_min_size(
        Pos2::new(
            viewport.right() - margin.x - size.x,
            viewport.top() + margin.y,
        ),
        size,
    ))
}

pub(super) fn paint(
    ui: &mut egui::Ui,
    editor: egui::Id,
    document: DocumentKey,
    read_only: bool,
    viewport: Rect,
) {
    let id = state_id(editor);
    if !read_only {
        ui.ctx().data_mut(|data| data.remove::<Rejection>(id));
        return;
    }
    let Some(mut rejection) = ui.ctx().data(|data| data.get_temp::<Rejection>(id)) else {
        return;
    };
    let now = ui.input(|input| input.time);
    let frame = (rejection.document == document)
        .then(|| sample(rejection.born, now))
        .flatten();
    let Some(frame) = frame else {
        ui.ctx().data_mut(|data| data.remove::<Rejection>(id));
        return;
    };
    if !viewport.is_finite() || !viewport.is_positive() {
        return;
    }
    let warning = theme::palette(ui.ctx()).warning;
    let painter = ui.painter().with_clip_rect(viewport);
    if frame.pulse > 0.0 {
        painter.rect_stroke(
            viewport.shrink(1.0),
            theme::RADIUS.row,
            Stroke::new(2.0, warning.linear_multiply(frame.pulse)),
            StrokeKind::Inside,
        );
    }
    let text_width = (viewport.width() - 2.0 * (MARGIN + PADDING.x)).max(1.0);
    let mut job = egui::text::LayoutJob::simple_singleline(
        MESSAGE.into(),
        egui::FontId::proportional(theme::TYPE.supporting),
        warning.linear_multiply(frame.opacity),
    );
    job.wrap.max_width = text_width;
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('…');
    let galley = painter.layout_job(job);
    let Some(rect) = badge_rect(viewport, galley.size()) else {
        return;
    };
    let background = ui.visuals().extreme_bg_color.linear_multiply(frame.opacity);
    painter.rect_filled(rect, theme::RADIUS.row, background);
    painter.rect_stroke(
        rect,
        theme::RADIUS.row,
        Stroke::new(1.0, warning.linear_multiply(frame.opacity)),
        StrokeKind::Inside,
    );
    painter.with_clip_rect(rect).galley(
        rect.min + PADDING,
        galley,
        warning.linear_multiply(frame.opacity),
    );
    let response = ui.interact(
        rect,
        editor.with("read-only-rejection-badge"),
        Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, MESSAGE));
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe("editor.read_only_rejection", &response);
    rejection.painted_frame = Some(ui.ctx().cumulative_frame_nr());
    ui.ctx().data_mut(|data| data.insert_temp(id, rejection));
    // Animate only the brief border pulse and final fade. The message's dwell
    // schedules one wakeup; expiry removes the state and stops its repaints.
    ui.ctx()
        .request_repaint_after(Duration::from_secs_f64(frame.wake_after));
}

#[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
pub(super) fn visible(context: &egui::Context, editor: egui::Id, document: DocumentKey) -> bool {
    context
        .data(|data| data.get_temp::<Rejection>(state_id(editor)))
        .is_some_and(|rejection| {
            rejection.document == document
                && rejection.painted_frame == Some(context.cumulative_frame_nr())
                && sample(rejection.born, context.input(|input| input.time)).is_some()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_requires_an_open_editable_document() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        for kind in [Some(DocumentKind::Pdf), Some(DocumentKind::Image), None] {
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            if let Some(kind) = kind {
                app.document_mut().replace_loaded_unprojected(
                    String::new(),
                    directory.path().join("asset"),
                    kind,
                    None,
                );
            } else {
                app.tabs = app.tabs.empty_after();
            }
            let key = app.document().key();
            app.insert_editor_text(&context, "hidden source mutation");
            app.insert_generated_symbol(
                &context,
                &crate::handwriting::Insertion {
                    text: "x".into(),
                    package: None,
                },
            );
            assert_eq!(app.document().source(), "", "{kind:?}");
            assert_eq!(app.document().key(), key, "{kind:?}");
            assert!(!app.document().is_dirty());
        }
    }

    #[test]
    fn rejection_envelope_is_finite_and_does_not_animate_during_the_message_dwell() {
        assert!(sample(10.0, 9.0).is_none());
        for now in [f64::NAN, f64::INFINITY, 10.0 + LIFETIME, 100.0] {
            assert!(sample(10.0, now).is_none());
        }
        let first = sample(10.0, 10.0).unwrap();
        assert_eq!(first.opacity, 1.0);
        assert_eq!(first.pulse, 1.0);
        let dwell = sample(10.0, 10.5).unwrap();
        assert_eq!(dwell.opacity, 1.0);
        assert_eq!(dwell.pulse, 0.0);
        assert!(
            dwell.wake_after > 0.8,
            "no frame loop while the badge is stationary"
        );
        let fade = sample(10.0, 11.5).unwrap();
        assert!(fade.opacity > 0.0 && fade.opacity < 1.0);
        assert_eq!(fade.pulse, 0.0);
    }

    #[test]
    fn rejection_is_coalesced_and_only_reports_the_document_and_frame_actually_painted() {
        let context = egui::Context::default();
        let editor = egui::Id::new("source");
        let document = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(7), 1, 0);
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 250.0));
        let output = context.run_ui(
            egui::RawInput {
                time: Some(1.0),
                ..Default::default()
            },
            |ui| {
                for _ in 0..100 {
                    reject(ui.ctx(), editor, document);
                }
                assert!(
                    !visible(ui.ctx(), editor, document),
                    "requesting feedback is not painting it"
                );
                paint(ui, editor, document, true, viewport);
                assert!(visible(ui.ctx(), editor, document));
                assert!(!visible(ui.ctx(), editor, document.after_edit()));
            },
        );
        output.drop_without_applying_deltas();
        assert!(
            !visible(&context, editor, document),
            "the inspector must not claim a stale frame was painted"
        );
        let output = context.run_ui(
            egui::RawInput {
                time: Some(1.1),
                ..Default::default()
            },
            |ui| {
                paint(ui, editor, document.after_edit(), true, viewport);
                assert!(!visible(ui.ctx(), editor, document));
                assert!(
                    ui.ctx()
                        .data(|data| data.get_temp::<Rejection>(state_id(editor)))
                        .is_none()
                );
            },
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn rejection_badge_stays_inside_small_offset_and_wide_viewports() {
        for origin in [Pos2::ZERO, Pos2::new(230.0, 190.0)] {
            for size in [
                Vec2::new(1.0, 1.0),
                Vec2::new(35.0, 20.0),
                Vec2::new(450.0, 700.0),
            ] {
                let viewport = Rect::from_min_size(origin, size);
                let rect = badge_rect(viewport, Vec2::new(240.0, 16.0)).unwrap();
                assert!(viewport.contains_rect(rect));
                assert!(rect.is_positive());
            }
        }
        assert!(badge_rect(Rect::NOTHING, Vec2::splat(10.0)).is_none());
    }
}
