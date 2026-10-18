//! One finite accent envelope; independent from the particle GPU and text layout.
use super::*;

pub(super) const LIFETIME: f64 = 0.48;
const SHAKE_SECONDS: f64 = 0.22;
const FLASH_SECONDS: f64 = 0.16;

#[derive(Clone, Copy)]
pub(super) struct Pulse {
    pub(super) origin: Pos2,
    pub(super) frame: u64,
    settings: ActionEffectSettings,
    born: f64,
}
#[derive(Clone, Copy, Default)]
pub(super) struct Frame {
    pub(super) shake: Vec2,
    pub(super) flash: f32,
    pub(super) glow: f32,
}
impl Pulse {
    pub(super) fn new(origin: Pos2, settings: ActionEffectSettings, born: f64, frame: u64) -> Self {
        Self {
            origin,
            frame,
            settings,
            born,
        }
    }
    pub(super) fn alive(self, now: f64) -> bool {
        let age = now - self.born;
        age.is_finite()
            && age >= 0.0
            && ((self.settings.shake && age < SHAKE_SECONDS)
                || (self.settings.flash && age < FLASH_SECONDS)
                || (self.settings.glow && age < LIFETIME))
    }
    pub(super) fn sample(self, now: f64) -> Frame {
        let age = now - self.born;
        if !age.is_finite() || age < 0.0 {
            return Frame::default();
        }
        let fade = |duration: f64| (1.0 - age / duration).clamp(0.0, 1.0) as f32;
        let shake = if self.settings.shake {
            let envelope = fade(SHAKE_SECONDS).powi(2);
            let phase = age as f32;
            egui::vec2(
                (phase * 97.0).sin() * 2.8 + (phase * 211.0).sin() * 0.8,
                (phase * 131.0).sin() * 2.4,
            ) * envelope
        } else {
            Vec2::ZERO
        };
        Frame {
            shake,
            flash: if self.settings.flash {
                fade(FLASH_SECONDS).powi(2) * 0.16
            } else {
                0.0
            },
            glow: if self.settings.glow {
                fade(LIFETIME).powi(2) * 0.42
            } else {
                0.0
            },
        }
    }
}

pub(super) fn paint(ui: &Ui, viewport: Rect, origin: Pos2, accent: egui::Color32, frame: Frame) {
    if !viewport.is_finite() || !viewport.is_positive() {
        return;
    }
    let painter = ui.painter().with_clip_rect(viewport);
    let tint = |alpha: f32| {
        egui::Color32::from_rgba_unmultiplied(
            accent.r(),
            accent.g(),
            accent.b(),
            (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    };
    if frame.flash > 0.0 {
        painter.rect_filled(viewport, 0.0, tint(frame.flash));
    }
    if frame.glow > 0.0 && origin.is_finite() {
        // Two small radial bands, independent of document length and DPI.
        const SEGMENTS: usize = 32;
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(origin, tint(frame.glow * 0.65));
        for (radius, alpha) in [(24.0, frame.glow), (96.0, 0.0)] {
            for index in 0..SEGMENTS {
                let angle = index as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                mesh.colored_vertex(origin + Vec2::angled(angle) * radius, tint(alpha));
            }
        }
        for index in 0..SEGMENTS as u32 {
            let next = (index + 1) % SEGMENTS as u32;
            mesh.add_triangle(0, 1 + index, 1 + next);
            mesh.add_triangle(1 + index, 33 + index, 33 + next);
            mesh.add_triangle(1 + index, 33 + next, 1 + next);
        }
        painter.add(egui::Shape::mesh(mesh));
    }
}

#[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
#[derive(Clone)]
struct Trace {
    identity: Identity,
    burst_count: u64,
    action_bursts: [u64; 10],
    last_action: Action,
    settings: ActionEffectSettings,
    last_emission: f64,
}
#[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
pub(super) fn record(
    context: &egui::Context,
    id: Id,
    identity: Identity,
    action: Action,
    settings: ActionEffectSettings,
    now: f64,
) {
    context.data_mut(|data| {
        let key = id.with("feedback-trace");
        let mut trace = data
            .get_temp::<Trace>(key)
            .filter(|t| t.identity.document == identity.document)
            .unwrap_or(Trace {
                identity,
                burst_count: 0,
                action_bursts: [0; 10],
                last_action: action,
                settings,
                last_emission: now,
            });
        trace.burst_count = trace.burst_count.saturating_add(1);
        trace.action_bursts[action as usize] =
            trace.action_bursts[action as usize].saturating_add(1);
        trace.identity = identity;
        trace.last_action = action;
        trace.settings = settings;
        trace.last_emission = now;
        data.insert_temp(key, trace);
    });
}
#[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
pub(super) fn inspected(
    context: &egui::Context,
    id: Id,
    identity: DocumentKey,
    now: f64,
) -> serde_json::Value {
    let (trace, pulse) = context.data(|data| {
        let trace = data
            .get_temp::<Trace>(id.with("feedback-trace"))
            .filter(|t| t.identity.document == identity);
        let pulse = data.get_temp::<SharedState>(id).and_then(|s| {
            let s = s.lock().unwrap();
            (s.identity.is_some_and(|key| key.document == identity))
                .then_some(s.feedback)
                .flatten()
        });
        (trace, pulse)
    });
    let names = |settings: ActionEffectSettings| {
        [
            ("Shake", settings.shake),
            ("Flash", settings.flash),
            ("Glow", settings.glow),
        ]
        .into_iter()
        .filter_map(|(name, on)| on.then_some(name))
        .collect::<Vec<_>>()
    };
    let frame = pulse.map_or(Frame::default(), |p| p.sample(now));
    let active = pulse.filter(|p| p.alive(now)).map_or_else(Vec::new, |p| {
        names(ActionEffectSettings {
            particles: None,
            shake: p.settings.shake && now - p.born < SHAKE_SECONDS,
            flash: frame.flash > 0.0,
            glow: frame.glow > 0.0,
        })
    });
    serde_json::json!({
        "burst_count": trace.as_ref().map_or(0, |t| t.burst_count),
        "action_bursts": Action::ALL.into_iter().filter_map(|action| {
            let count = trace.as_ref().map_or(0, |t| t.action_bursts[action as usize]);
            (count > 0).then_some((action.label(), count))
        }).collect::<std::collections::BTreeMap<_, _>>(),
        "last_action": trace.as_ref().map(|t| t.last_action.label()),
        "last_effects": trace.as_ref().map_or_else(Vec::new, |t| names(t.settings)),
        "active_effects": active, "shake_offset": [frame.shake.x, frame.shake.y],
        "flash_alpha": frame.flash, "glow_alpha": frame.glow,
        "lifetime_ms": (LIFETIME * 1000.0) as u64,
        "last_emission_age_ms": trace.as_ref().map(|t| (now - t.last_emission).max(0.0) * 1000.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn effects() -> ActionEffectSettings {
        ActionEffectSettings {
            particles: None,
            shake: true,
            flash: true,
            glow: true,
        }
    }
    #[test]
    fn finite_bounded_envelopes_expire_and_reject_invalid_time() {
        let pulse = Pulse::new(Pos2::ZERO, effects(), 5.0, 1);
        for i in 0..480 {
            let frame = pulse.sample(5.0 + i as f64 / 1000.0);
            assert!(frame.shake.x.abs() <= 4.0 && frame.shake.y.abs() <= 4.0);
            assert!((0.0..=0.16).contains(&frame.flash) && (0.0..=0.42).contains(&frame.glow));
        }
        for now in [4.0, 5.48, 100.0, f64::NAN, f64::INFINITY] {
            assert!(!pulse.alive(now));
            let frame = pulse.sample(now);
            assert_eq!(frame.shake, Vec2::ZERO);
            assert_eq!(frame.flash, 0.0);
            assert_eq!(frame.glow, 0.0);
        }
    }
    #[test]
    fn overlays_have_constant_work_and_keep_the_editor_clip() {
        let context = egui::Context::default();
        let clip = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(300.0, 200.0));
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            paint(
                ui,
                clip,
                clip.center(),
                egui::Color32::RED,
                Frame {
                    flash: 0.1,
                    glow: 0.3,
                    ..Default::default()
                },
            );
        });
        let meshes: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some((shape.clip_rect, mesh)),
                _ => None,
            })
            .collect();
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].0, clip);
        assert_eq!(meshes[0].1.vertices.len(), 65);
        assert_eq!(meshes[0].1.indices.len(), 288);
        output.drop_without_applying_deltas();
    }
    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "fresh offscreen editor-feedback pixel review"]
    fn feedback_framebuffer_review() {
        for dark in [false, true] {
            let context = egui::Context::default();
            theme::configure_test_fonts(&context);
            context.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut framebuffer = crate::test_framebuffer::Framebuffer::new([720, 380]);
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(720.0, 380.0))),
                    ..Default::default()
                },
                |ui| {
                    let background = if dark {
                        egui::Color32::from_rgb(30, 30, 46)
                    } else {
                        egui::Color32::from_rgb(239, 241, 245)
                    };
                    let ink = if dark {
                        egui::Color32::from_rgb(205, 214, 244)
                    } else {
                        egui::Color32::from_rgb(76, 79, 105)
                    };
                    let accent = if dark {
                        egui::Color32::from_rgb(203, 166, 247)
                    } else {
                        egui::Color32::from_rgb(136, 57, 239)
                    };
                    ui.painter().rect_filled(ui.max_rect(), 0.0, background);
                    for (index, name) in ["Shake", "Flash", "Glow"].into_iter().enumerate() {
                        let clip = Rect::from_min_size(
                            egui::pos2(16.0 + index as f32 * 234.0, 16.0),
                            egui::vec2(220.0, 348.0),
                        );
                        ui.painter().rect_stroke(
                            clip,
                            0.0,
                            egui::Stroke::new(1.0, ink.gamma_multiply(0.3)),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            clip.min + egui::vec2(10.0, 10.0),
                            egui::Align2::LEFT_TOP,
                            name,
                            egui::FontId::proportional(16.0),
                            ink,
                        );
                        let marker = shake::begin(ui);
                        for row in 0..10 {
                            ui.painter().with_clip_rect(clip).text(
                                clip.min + egui::vec2(10.0, 56.0 + row as f32 * 24.0),
                                egui::Align2::LEFT_TOP,
                                format!("{:2}  #let x = {}", row + 1, row),
                                egui::FontId::monospace(14.0),
                                ink,
                            );
                        }
                        let range = marker.finish(ui);
                        let pulse = Pulse::new(
                            clip.min + egui::vec2(135.0, 164.0),
                            ActionEffectSettings {
                                particles: None,
                                shake: name == "Shake",
                                flash: name == "Flash",
                                glow: name == "Glow",
                            },
                            0.0,
                            0,
                        );
                        let frame = pulse.sample(0.035);
                        range.apply(ui, frame.shake, clip);
                        paint(ui, clip, pulse.origin, accent, frame);
                    }
                },
            );
            let path = std::path::PathBuf::from(format!(
                ".tiptoptyp/screenshots/agent-review/editor-effects/{}-feedback.png",
                if dark { "dark" } else { "light" }
            ));
            framebuffer.render(&context, output, Some(&path));
            assert!(path.metadata().unwrap().len() > 0);
        }
    }
}
