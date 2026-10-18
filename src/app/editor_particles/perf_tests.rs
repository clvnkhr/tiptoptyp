//! Opt-in effects CPU submission probe; this is not a key-to-pixel or GPU benchmark.
use super::*;
use eframe::egui::{Color32, FontId};
use std::{hint::black_box, time::Instant};
use tiptoptyp_core::document::WindowSessionId;

#[derive(Clone, Copy, Debug)]
enum Workload {
    Off,
    Idle,
    Particles,
    Shake,
    Flash,
    Glow,
    All,
}

impl Workload {
    fn settings(self, frame: usize) -> EditorEffectSettings {
        let mut settings = EditorEffectSettings {
            enabled: !matches!(self, Self::Off),
            density: ParticleDensity::High,
            ..Default::default()
        };
        settings.actions.typing = ActionEffectSettings {
            particles: matches!(self, Self::Idle | Self::Particles | Self::All).then_some(
                [
                    ParticleStyle::Explosion,
                    ParticleStyle::Shotgun,
                    ParticleStyle::Flamethrower,
                    ParticleStyle::Railgun,
                    ParticleStyle::Plasma,
                ][(frame / 3) % 5],
            ),
            shake: matches!(self, Self::Shake | Self::All),
            flash: matches!(self, Self::Flash | Self::All),
            glow: matches!(self, Self::Glow | Self::All),
        };
        settings
    }

    fn emits(self) -> bool {
        !matches!(self, Self::Off | Self::Idle)
    }
}

fn percentile(samples: &[f64], percent: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() - 1) * percent / 100]
}

#[test]
#[ignore = "opt-in matched optimized effects CPU submission timing; excludes GPU and editor layout"]
fn editor_effect_cpu_cost_probe() {
    const WARMUP: usize = 20;
    const SAMPLES: usize = 180;
    const SEED: &str = "  let amplitude = sin(time) + math.alpha; // finite paint fixture\n";
    assert!(
        !black_box(cfg!(debug_assertions)),
        "run this probe with --release"
    );
    for lines in [1_000, 20_000] {
        for workload in [
            Workload::Off,
            Workload::Idle,
            Workload::Particles,
            Workload::Shake,
            Workload::Flash,
            Workload::Glow,
            Workload::All,
        ] {
            let context = egui::Context::default();
            context.set_theme(egui::Theme::Dark);
            let mut source = SEED.repeat(lines);
            let bytes = source.len();
            let editor = Id::new("effects-cost-probe-editor");
            let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(760.0, 600.0));
            let mut editor_state = egui::text_edit::TextEditState::default();
            editor_state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(2),
                )));
            editor_state.store(&context, editor);
            let mut cached_galley = None;
            let mut samples = Vec::with_capacity(SAMPLES);
            let mut shapes = 0_usize;
            for frame in 0..WARMUP + SAMPLES {
                let mut elapsed = 0.0;
                let output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(800.0, 640.0),
                        )),
                        time: Some(frame as f64 / 120.0),
                        focused: true,
                        ..Default::default()
                    },
                    |ui| {
                        ui.set_clip_rect(viewport);
                        ui.memory_mut(|memory| memory.request_focus(editor));
                        let settings = workload.settings(frame);
                        let marker_started = Instant::now();
                        let marker = prepare_shake(ui, editor, settings);
                        let marker_elapsed = marker_started.elapsed();
                        // Layout/font lookup and the editor widget are outside the timed
                        // scope. Its real TextShape retains the complete cached document,
                        // so shake must not copy or traverse its off-screen galley rows.
                        let mut layouter = |ui: &Ui, buffer: &dyn egui::TextBuffer, _: f32| {
                            cached_galley
                                .get_or_insert_with(|| {
                                    ui.painter().layout_no_wrap(
                                        buffer.as_str().to_owned(),
                                        FontId::monospace(14.0),
                                        Color32::WHITE,
                                    )
                                })
                                .clone()
                        };
                        let edit = egui::TextEdit::multiline(&mut source)
                            .id(editor)
                            .frame(egui::Frame::NONE)
                            .desired_width(viewport.width())
                            .layouter(&mut layouter)
                            .show(ui);
                        let finish_started = Instant::now();
                        let paint_range = marker.map(|marker| marker.finish(ui));
                        let marker_elapsed = marker_elapsed + finish_started.elapsed();
                        let emits = workload.emits() && frame % 3 == 0;
                        let started = Instant::now();
                        paint(
                            ui,
                            Surface {
                                edit: &edit,
                                document: DocumentKey::new(
                                    WindowSessionId::new(1),
                                    1,
                                    frame as u64,
                                ),
                                viewport,
                                view: View::Source,
                                paint_range,
                                shake_viewport: viewport,
                            },
                            settings,
                            InputIntent {
                                events: u8::from(emits),
                                commands_enabled: true,
                                ..Default::default()
                            },
                            emits,
                        );
                        elapsed = (marker_elapsed + started.elapsed()).as_secs_f64() * 1_000_000.0;
                    },
                );
                if frame >= WARMUP {
                    samples.push(elapsed);
                    shapes += output.shapes.len();
                }
                black_box(&output.shapes);
                output.drop_without_applying_deltas();
            }
            let observed = inspected(
                &context,
                editor,
                DocumentKey::new(WindowSessionId::new(1), 1, 0),
            );
            let expected_bursts = if workload.emits() {
                (WARMUP + SAMPLES).div_ceil(3) as u64
            } else {
                0
            };
            assert_eq!(
                observed["burst_count"].as_u64().unwrap(),
                if matches!(workload, Workload::Particles | Workload::All) {
                    expected_bursts
                } else {
                    0
                },
                "{workload:?}: the measured particle path must actually emit"
            );
            assert_eq!(
                observed["feedback"]["burst_count"].as_u64().unwrap(),
                if matches!(
                    workload,
                    Workload::Shake | Workload::Flash | Workload::Glow | Workload::All
                ) {
                    expected_bursts
                } else {
                    0
                },
                "{workload:?}: the measured feedback path must actually emit"
            );
            assert!(observed["max_live"].as_u64().unwrap() <= LIMIT as u64);
            eprintln!(
                "editor_effect_cpu workload={workload:?} lines={lines} fixture_bytes={bytes} viewport=800x640 effect_viewport=760x600 font=monospace14 theme=egui-dark scale=1 logical_hz=120 action_hz={} density=High warmup={WARMUP} frames={SAMPLES} p50_us={:.3} p95_us={:.3} mean_us={:.3} shapes_mean={:.1}; excludes_layout_tessellation_gpu=true",
                if workload.emits() { 40 } else { 0 },
                percentile(&samples, 50),
                percentile(&samples, 95),
                samples.iter().sum::<f64>() / SAMPLES as f64,
                shapes as f64 / SAMPLES as f64,
            );
        }
    }
}
