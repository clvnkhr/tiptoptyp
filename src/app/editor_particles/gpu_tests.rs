#![allow(unsafe_code)]
//! Real renderer coverage, kept separate from the particle simulation tests.
use super::gpu::Renderer;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use std::sync::Arc;

fn instances() -> Arc<[[f32; 8]]> {
    let mut state = super::State::default();
    state.update(
        super::Identity::new(
            crate::document::DocumentKey::new(
                tiptoptyp_core::document::WindowSessionId::new(1),
                1,
                0,
            ),
            super::View::Source,
        ),
        0,
        0.0,
        Some(egui::pos2(79.0, 94.0)),
        super::density_count(crate::settings::ParticleDensity::Medium),
    );
    state.instances()
}

#[test]
fn empty_or_clipped_gpu_batches_do_not_add_render_callbacks() {
    let context = egui::Context::default();
    let renderer = Renderer::for_context(&context);
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(360.0, 240.0))),
            ..Default::default()
        },
        |ui| {
            renderer.submit(ui, ui.clip_rect(), Arc::from([]), 0.0, Color32::RED, false);
            renderer.submit(ui, Rect::NOTHING, instances(), 0.0, Color32::RED, false);
            renderer.submit(
                ui,
                ui.clip_rect(),
                vec![[0.0; 8]; super::LIMIT + 1].into(),
                0.0,
                Color32::RED,
                false,
            );
            for time in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                renderer.submit(ui, ui.clip_rect(), instances(), time, Color32::RED, false);
            }
            renderer.submit(
                ui,
                Rect::from_min_max(Pos2::ZERO, egui::pos2(f32::INFINITY, 100.0)),
                instances(),
                0.0,
                Color32::RED,
                false,
            );
        },
    );
    assert!(
        output
            .shapes
            .iter()
            .all(|shape| !matches!(shape.shape, egui::Shape::Callback(_))),
        "empty GPU work must not enter the renderer"
    );
    let observed = renderer.inspected();
    assert_eq!(observed["draws"], 0);
    assert_eq!(observed["uploads"], 0);
    output.drop_without_applying_deltas();
}

#[cfg(target_os = "macos")]
fn render_frame(
    framebuffer: &mut crate::test_framebuffer::Framebuffer,
    context: &egui::Context,
    renderer: &Renderer,
    batch: Arc<[[f32; 8]]>,
    appearance: (bool, f32, u8),
    time: f32,
    path: Option<&std::path::Path>,
) -> egui::ColorImage {
    let (dark, scale, opacity) = appearance;
    let mut input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(360.0, 240.0))),
        time: Some(f64::from(time)),
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(scale);
    let output = context.run_ui(input, |ui| {
        let background = if dark {
            Color32::from_rgb(30, 30, 46)
        } else {
            Color32::from_rgb(239, 241, 245)
        };
        let accent = if dark {
            Color32::from_rgb(203, 166, 247)
        } else {
            Color32::from_rgb(136, 57, 239)
        };
        ui.painter().rect_filled(ui.max_rect(), 0.0, background);
        let clip = Rect::from_min_max(egui::pos2(64.0, 52.0), egui::pos2(296.0, 188.0));
        ui.painter().rect_filled(
            Rect::from_min_max(clip.min + Vec2::splat(6.0), clip.max - Vec2::splat(6.0)),
            0.0,
            if dark {
                Color32::from_rgb(36, 36, 52)
            } else {
                Color32::from_rgb(249, 249, 253)
            },
        );
        if opacity < 255 {
            // Framebuffer::render clears opaque white. Replace this clip's
            // backing pixels directly so the compositor-alpha test cannot
            // accidentally pass by blending a translucent rect over white.
            let alpha = f32::from(opacity) / 255.0;
            let clear = [
                f32::from(background.r()) / 255.0 * alpha,
                f32::from(background.g()) / 255.0 * alpha,
                f32::from(background.b()) / 255.0 * alpha,
                alpha,
            ];
            ui.painter().with_clip_rect(clip).add(egui::PaintCallback {
                rect: clip,
                callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_, painter| {
                    use eframe::glow::{self, HasContext as _};
                    unsafe {
                        painter
                            .gl()
                            .clear_color(clear[0], clear[1], clear[2], clear[3]);
                        painter.gl().clear(glow::COLOR_BUFFER_BIT);
                    }
                })),
            });
        }
        if batch.is_empty() {
            renderer.warm(ui, clip);
        }
        renderer.submit(ui, clip, batch.clone(), time, accent, dark);
        // Egui geometry after the custom callback must retain its normal
        // texture, blend and clip behavior. It also exposes an altered FBO.
        ui.painter().rect_filled(
            Rect::from_min_size(egui::pos2(312.0, 30.0), Vec2::splat(20.0)),
            0.0,
            Color32::from_rgb(240, 40, 170),
        );
    });
    framebuffer.render(context, output, path);
    framebuffer.read_rgba()
}

#[cfg(target_os = "macos")]
fn counter(renderer: &Renderer, key: &str) -> u64 {
    renderer.inspected()[key]
        .as_u64()
        .unwrap_or_else(|| panic!("missing GPU counter: {key}"))
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "real offscreen Glow explosion pixels and bounded GPU upload review"]
fn gpu_explosion_framebuffer_review() {
    use eframe::glow::{self, HasContext as _};
    for dark in [false, true] {
        for scale in [1.0, 2.0] {
            let context = egui::Context::default();
            let renderer = Renderer::for_context(&context);
            let mut framebuffer = crate::test_framebuffer::Framebuffer::new([
                (360.0 * scale) as u32,
                (240.0 * scale) as u32,
            ]);
            let gl = framebuffer.gl();
            let empty: Arc<[[f32; 8]]> = Arc::from([]);
            let batch = instances();
            let path = |phase: &str| {
                std::path::PathBuf::from(format!(
                    ".tiptoptyp/screenshots/agent-review/typing-particles/{}-{}x-{phase}.png",
                    if dark { "dark" } else { "light" },
                    scale as u32,
                ))
            };
            // Warm the same framebuffer/context before the matched pair.
            render_frame(
                &mut framebuffer,
                &context,
                &renderer,
                empty.clone(),
                (dark, scale, 255),
                0.0,
                None,
            );
            assert!(renderer.inspected()["ready"].as_bool().unwrap_or(false));
            assert_eq!(counter(&renderer, "draws"), 0);
            assert_eq!(counter(&renderer, "uploads"), 0);
            let baseline = render_frame(
                &mut framebuffer,
                &context,
                &renderer,
                empty.clone(),
                (dark, scale, 255),
                0.01,
                None,
            );
            let active = render_frame(
                &mut framebuffer,
                &context,
                &renderer,
                batch.clone(),
                (dark, scale, 255),
                0.14,
                Some(&path("active")),
            );
            assert!(
                renderer.inspected()["ready"].as_bool().unwrap_or(false),
                "{:?}",
                renderer.inspected()
            );
            assert_eq!(counter(&renderer, "uploads"), 1);
            assert_eq!(counter(&renderer, "uploaded_instances"), batch.len() as u64);
            assert_eq!(counter(&renderer, "draws"), 1);
            let mut changed = 0;
            let mut accent_colored = 0;
            for (index, (before, after)) in baseline.pixels.iter().zip(&active.pixels).enumerate() {
                if before == after {
                    continue;
                }
                changed += 1;
                let x = (index % active.size[0]) as f32 / scale;
                let y = (index / active.size[0]) as f32 / scale;
                assert!(
                    (64.0..296.0).contains(&x) && (52.0..188.0).contains(&y),
                    "GPU pixels escaped clip at {x},{y}"
                );
                if i16::from(after.b()) > i16::from(after.g()) + 8 {
                    accent_colored += 1;
                }
            }
            assert!(
                changed > (40.0 * scale * scale) as usize,
                "the explosion was absent: {changed} changed pixels"
            );
            assert!(
                accent_colored > (10.0 * scale * scale) as usize,
                "the explosion lost its theme accent"
            );
            let later = render_frame(
                &mut framebuffer,
                &context,
                &renderer,
                batch.clone(),
                (dark, scale, 255),
                0.30,
                Some(&path("later")),
            );
            assert_ne!(
                active.pixels, later.pixels,
                "GPU time must advance the explosion without uploading instances again"
            );
            assert_eq!(counter(&renderer, "uploads"), 1);
            assert_eq!(counter(&renderer, "draws"), 2);
            let expired = render_frame(
                &mut framebuffer,
                &context,
                &renderer,
                batch.clone(),
                (dark, scale, 255),
                super::LIFETIME as f32 + 0.1,
                None,
            );
            assert_eq!(
                baseline.pixels, expired.pixels,
                "expired particles must leave no residual pixels"
            );
            let draws = counter(&renderer, "draws");
            render_frame(
                &mut framebuffer,
                &context,
                &renderer,
                empty,
                (dark, scale, 255),
                super::LIFETIME as f32 + 0.2,
                None,
            );
            assert_eq!(
                counter(&renderer, "draws"),
                draws,
                "idle batches must not draw"
            );
            assert_eq!(
                counter(&renderer, "uploads"),
                1,
                "idle batches must not upload"
            );
            unsafe {
                assert_eq!(gl.get_error(), glow::NO_ERROR);
            }
            renderer.destroy(&gl);
            renderer.destroy(&gl);
            let retired = renderer.inspected();
            assert_eq!(retired["ready"], false);
            let after_destroy = render_frame(
                &mut framebuffer,
                &context,
                &renderer,
                batch,
                (dark, scale, 255),
                super::LIFETIME as f32 + 0.3,
                None,
            );
            assert_eq!(baseline.pixels, after_destroy.pixels);
            assert_eq!(
                renderer.inspected(),
                retired,
                "late submissions must be inert after cleanup"
            );
            unsafe {
                assert_eq!(gl.get_error(), glow::NO_ERROR);
            }
        }
    }
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "real Glow compositor-alpha contract for translucent particle backgrounds"]
fn gpu_translucent_background_preserves_valid_premultiplied_pixels() {
    use eframe::glow::{self, HasContext as _};
    for dark in [false, true] {
        for opacity in [0, 96] {
            let context = egui::Context::default();
            let renderer = Renderer::for_context(&context);
            let mut framebuffer = crate::test_framebuffer::Framebuffer::new([360, 240]);
            let gl = framebuffer.gl();
            let baseline = render_frame(
                &mut framebuffer,
                &context,
                &renderer,
                Arc::from([]),
                (dark, 1.0, opacity),
                0.0,
                None,
            );
            let path = std::path::PathBuf::from(format!(
                ".tiptoptyp/screenshots/agent-review/typing-particles/{}-alpha{opacity}-active.png",
                if dark { "dark" } else { "light" },
            ));
            let active = render_frame(
                &mut framebuffer,
                &context,
                &renderer,
                instances(),
                (dark, 1.0, opacity),
                0.035,
                Some(&path),
            );
            assert!(renderer.inspected()["ready"].as_bool().unwrap_or(false));
            assert_eq!(counter(&renderer, "draws"), 1);
            let mut changed = 0;
            let mut alpha_increased = 0;
            for (index, (before, after)) in baseline.pixels.iter().zip(&active.pixels).enumerate() {
                let x = index % active.size[0];
                let y = index / active.size[0];
                if !(64..296).contains(&x) || !(52..188).contains(&y) {
                    assert_eq!(before, after, "translucent GPU effect escaped its clip");
                    continue;
                }
                assert_eq!(
                    before.a(),
                    opacity,
                    "fixture must start translucent, not opaque"
                );
                for channel in [after.r(), after.g(), after.b()] {
                    assert!(
                        u16::from(channel) <= u16::from(after.a()) + 1,
                        "invalid premultiplied GPU pixel at {x},{y}: {:?}",
                        after.to_array(),
                    );
                }
                if dark {
                    assert!(
                        after.a() >= before.a(),
                        "additive glow reduced backing alpha"
                    );
                    alpha_increased += usize::from(after.a() > before.a());
                } else {
                    assert_eq!(
                        after.a(),
                        before.a(),
                        "multiply-atop must preserve backing alpha"
                    );
                    if opacity == 0 {
                        assert_eq!(
                            after.to_array(),
                            [0; 4],
                            "transparent light pixels became black compositor stains"
                        );
                    }
                }
                changed += usize::from(before != after);
            }
            if !dark && opacity == 0 {
                assert_eq!(active.pixels, baseline.pixels);
            } else {
                assert!(
                    changed > 10,
                    "the alpha contract must exercise a visible effect"
                );
            }
            if dark {
                assert!(
                    alpha_increased > 10,
                    "additive glow needs alpha to cover its emitted RGB"
                );
            }
            renderer.destroy(&gl);
            unsafe {
                assert_eq!(gl.get_error(), glow::NO_ERROR);
            }
        }
    }
}
