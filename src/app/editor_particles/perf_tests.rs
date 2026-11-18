//! Opt-in effects CPU submission probe; this is not a key-to-pixel or GPU benchmark.
use super::super::editor_layout::CursorIndex;
use super::*;
use eframe::egui::{Color32, FontId, Galley, text::CCursor};
use std::{hint::black_box, time::Instant};
use tiptoptyp_core::document::WindowSessionId;

#[derive(Clone, Copy, Debug)]
enum Workload {
    Off,
    Idle,
    Particles,
    Smoke,
    Afterburner,
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
            cursor_afterburner: matches!(self, Self::Afterburner),
            ..Default::default()
        };
        settings.actions.typing = ActionEffectSettings {
            lingering: matches!(self, Self::Smoke).then_some(LingeringStyle::Smoke),
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
        !matches!(self, Self::Off | Self::Idle | Self::Afterburner)
    }
}

#[derive(Clone, Copy, Debug)]
enum Position {
    Start,
    NearEof,
}

impl Position {
    fn caret(self, character_count: usize, frame: usize, workload: Workload) -> usize {
        let index = match self {
            Self::Start => 2,
            Self::NearEof => character_count - 3,
        };
        index - usize::from(matches!(workload, Workload::Afterburner) && frame % 2 == 1)
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
        for position in [Position::Start, Position::NearEof] {
            for workload in [
                Workload::Off,
                Workload::Idle,
                Workload::Particles,
                Workload::Smoke,
                Workload::Afterburner,
                Workload::Shake,
                Workload::Flash,
                Workload::Glow,
                Workload::All,
            ] {
                let context = egui::Context::default();
                clock::Clock::use_manual(&context);
                context.set_theme(egui::Theme::Dark);
                let mut source = SEED.repeat(lines);
                let bytes = source.len();
                let character_count = source.chars().count();
                let editor = Id::new("effects-cost-probe-editor");
                let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(760.0, 600.0));
                let mut editor_state = egui::text_edit::TextEditState::default();
                editor_state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(CCursor::new(
                        position.caret(character_count, 0, workload),
                    ))));
                editor_state.store(&context, editor);
                let mut cached_galley = None;
                let mut cached_cursor_index: Option<(Arc<Galley>, CursorIndex)> = None;
                let mut samples = Vec::with_capacity(SAMPLES);
                let mut shapes = 0_usize;
                for frame in 0..WARMUP + SAMPLES {
                    let cursor = CCursor::new(position.caret(character_count, frame, workload));
                    let mut editor_state =
                        egui::text_edit::TextEditState::load(&context, editor).unwrap_or_default();
                    editor_state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(cursor)));
                    editor_state.store(&context, editor);
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
                            let mut edit = egui::TextEdit::multiline(&mut source)
                                .id(editor)
                                .frame(egui::Frame::NONE)
                                .desired_width(viewport.width())
                                .layouter(&mut layouter)
                                .show(ui);
                            if cached_cursor_index
                                .as_ref()
                                .is_none_or(|(galley, _)| !Arc::ptr_eq(galley, &edit.galley))
                            {
                                cached_cursor_index = Some((
                                    Arc::clone(&edit.galley),
                                    CursorIndex::new(&edit.galley),
                                ));
                            }
                            let cursor_index = &cached_cursor_index.as_ref().unwrap().1;
                            if matches!(position, Position::NearEof) {
                                // This component probe does not time a scrolling parent.
                                // Translate its reported galley origin to put the near-EOF
                                // caret inside the effect viewport, while retaining every
                                // document row in the cached layout and shake paint.
                                edit.galley_pos.y = viewport.center().y
                                    - cursor_index.rect(&edit.galley, cursor).center().y;
                            }
                            let finish_started = Instant::now();
                            let paint_range = marker.map(|marker| marker.finish(ui));
                            let marker_elapsed = marker_elapsed + finish_started.elapsed();
                            let emits = workload.emits() && frame % 3 == 0;
                            let started = Instant::now();
                            paint(
                                ui,
                                Surface {
                                    edit: &edit,
                                    cursor_index,
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
                            elapsed =
                                (marker_elapsed + started.elapsed()).as_secs_f64() * 1_000_000.0;
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
                    if matches!(
                        workload,
                        Workload::Particles | Workload::Smoke | Workload::All
                    ) {
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
                assert_eq!(
                    observed["cursor_bursts"].as_u64().unwrap(),
                    if matches!(workload, Workload::Afterburner) {
                        (WARMUP + SAMPLES - 1) as u64
                    } else {
                        0
                    },
                    "{workload:?}: every measured navigation after the baseline must emit"
                );
                assert!(observed["max_live"].as_u64().unwrap() <= LIMIT as u64);
                eprintln!(
                    "editor_effect_cpu workload={workload:?} position={position:?} lines={lines} fixture_bytes={bytes} viewport=800x640 effect_viewport=760x600 font=monospace14 theme=egui-dark scale=1 logical_hz=120 action_hz={} cursor_hz={} density=High warmup={WARMUP} frames={SAMPLES} p50_us={:.3} p95_us={:.3} mean_us={:.3} shapes_mean={:.1}; excludes_layout_tessellation_gpu=true",
                    if workload.emits() { 40 } else { 0 },
                    if matches!(workload, Workload::Afterburner) {
                        120
                    } else {
                        0
                    },
                    percentile(&samples, 50),
                    percentile(&samples, 95),
                    samples.iter().sum::<f64>() / SAMPLES as f64,
                    shapes as f64 / SAMPLES as f64,
                );
            }
        }
    }
}

fn timed_cursor_lookups(mut locate: impl FnMut() -> Rect, iterations: usize) -> f64 {
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(locate());
    }
    started.elapsed().as_secs_f64() * 1_000_000.0 / iterations as f64
}

#[test]
#[ignore = "optimized paired cursor geometry component timings; excludes index construction, layout, widgets and GPU"]
fn editor_effect_cursor_lookup_cost_probe() {
    use sha2::{Digest, Sha256};

    assert!(
        !black_box(cfg!(debug_assertions)),
        "run this probe with --release"
    );
    const WARMUP: usize = 32;
    const ITERATIONS: usize = 512;
    const ROUNDS: usize = 5;
    const SEED: &str = "  αβ量e\u{301} + x => y; // wrap boundary\n";
    for lines in [1_000, 20_000] {
        let context = egui::Context::default();
        let source = SEED.repeat(lines);
        let characters = source.chars().count();
        let fixture_sha256 = hex::encode(Sha256::digest(source.as_bytes()));
        let mut layouts = Vec::new();
        context
            .run_ui(Default::default(), |ui| {
                for (label, width) in [("unwrapped", f32::INFINITY), ("wrapped", 150.0)] {
                    let galley = ui.painter().layout(
                        source.clone(),
                        FontId::monospace(14.0),
                        Color32::WHITE,
                        width,
                    );
                    layouts.push((label, galley));
                }
            })
            .drop_without_applying_deltas();
        // Synthetic final folded geometry preserves all character counts,
        // like the production projection. Rendering/layout is outside this
        // lookup-only probe; compare both algorithms against this same galley.
        let mut folded = (*layouts[1].1).clone();
        let mut hidden_height = 0.0;
        for (index, row) in folded.rows.iter_mut().enumerate() {
            row.pos.y -= hidden_height;
            if (3..6).contains(&(index % 7)) {
                hidden_height += row.size.y;
                let contents = Arc::make_mut(&mut row.row);
                contents.size = Vec2::ZERO;
                contents.visuals = Default::default();
            }
        }
        layouts.push(("wrapped-folded", Arc::new(folded)));
        for (layout, galley) in layouts {
            let index_started = Instant::now();
            let index = CursorIndex::new(&galley);
            let index_build_us = index_started.elapsed().as_secs_f64() * 1_000_000.0;
            for (position, character) in [
                ("start", 2),
                ("middle", characters / 2),
                ("near-eof", characters - 3),
            ] {
                for prefer_next_row in [false, true] {
                    let cursor = CCursor {
                        index: character.into(),
                        prefer_next_row,
                    };
                    assert_eq!(
                        index.rect(&galley, cursor),
                        galley.pos_from_cursor(cursor),
                        "{lines} {layout} {position} {prefer_next_row}"
                    );
                    for _ in 0..WARMUP {
                        black_box(galley.pos_from_cursor(black_box(cursor)));
                        black_box(index.rect(&galley, black_box(cursor)));
                    }
                    let mut direct_us = Vec::with_capacity(ROUNDS);
                    let mut indexed_us = Vec::with_capacity(ROUNDS);
                    for round in 0..ROUNDS {
                        // Alternate order to reduce a systematic warm-cache or
                        // thermal advantage. Use identical immutable layouts.
                        let direct = || {
                            timed_cursor_lookups(
                                || black_box(&galley).pos_from_cursor(black_box(cursor)),
                                ITERATIONS,
                            )
                        };
                        let indexed = || {
                            timed_cursor_lookups(
                                || black_box(&index).rect(black_box(&galley), black_box(cursor)),
                                ITERATIONS,
                            )
                        };
                        let (reference, candidate) = if round % 2 == 0 {
                            (direct(), indexed())
                        } else {
                            let candidate = indexed();
                            (direct(), candidate)
                        };
                        direct_us.push(reference);
                        indexed_us.push(candidate);
                    }
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "measurement": "editor_effect_cursor_lookup",
                            "build": crate::build_info::VERSION,
                            "os": std::env::consts::OS,
                            "arch": std::env::consts::ARCH,
                            "optimized": !cfg!(debug_assertions),
                            "logical_lines": lines,
                            "visual_rows": galley.rows.len(),
                            "fixture_bytes": source.len(),
                            "fixture_sha256": fixture_sha256,
                            "layout": layout,
                            "position": position,
                            "char_index": character,
                            "prefer_next_row": prefer_next_row,
                            "font": "egui monospace14",
                            "wrapped_width": 150.0,
                            "warmup_pairs": WARMUP,
                            "rounds": ROUNDS,
                            "iterations_per_sample": ITERATIONS,
                            "direct_us_per_lookup": direct_us,
                            "indexed_us_per_lookup": indexed_us,
                            "index_build_us_single_sample": index_build_us,
                            "excludes": ["layout", "index_construction", "editor_widget", "tessellation", "gpu"],
                        })
                    );
                }
            }
        }
    }
}
