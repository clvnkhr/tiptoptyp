//! Deterministic visual inventory of production icon painters.
use super::icons::{UiIcon, paint_ui_icon};
use crate::theme::METRICS;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

/// Native framebuffer audit: real painters at 14 px and exactly four times
/// that geometry/stroke weight. Labels stay readable at normal size.
pub(super) fn show(ui: &mut egui::Ui) {
    let ctx = ui.ctx();
    ctx.set_pixels_per_point(1.0);
    let sheet_size = Vec2::new(1400.0, 900.0);
    if ctx.input(|input| input.viewport_rect().size()) != sheet_size {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(sheet_size));
    }
    let foreground = Color32::from_rgb(54, 61, 77);
    let background = Color32::from_rgb(248, 249, 252);
    ui.painter().rect_filled(ui.max_rect(), 0.0, background);
    let text = |position, size, value: &str| {
        ui.painter().text(
            position,
            egui::Align2::LEFT_TOP,
            value,
            egui::FontId::proportional(size),
            foreground,
        );
    };
    text(egui::pos2(24.0, 16.0), 22.0, "tiptoptyp / icon audit");
    text(
        egui::pos2(24.0, 44.0),
        13.0,
        "Each pair: 1× (14 px) + 4× (56 px). Native vector rendering; no bitmap enlargement.",
    );
    for (index, icon) in UiIcon::ALL.iter().copied().enumerate() {
        let origin = egui::pos2(
            24.0 + (index % 6) as f32 * 220.0,
            76.0 + (index / 6) as f32 * 88.0,
        );
        text(origin, 12.0, &format!("{icon:?}"));
        for (slot, scale) in [1.0, 4.0].into_iter().enumerate() {
            let layer = egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new(("icon-audit", index, slot)),
            );
            let painter = ctx.layer_painter(layer).with_clip_rect(Rect::EVERYTHING);
            let size = if matches!(icon, UiIcon::Folder | UiIcon::File) {
                METRICS.explorer.tree_icon_size
            } else {
                Vec2::splat(14.0)
            };
            paint_ui_icon(
                &painter,
                Rect::from_center_size(egui::pos2(7.0, 7.0), size),
                icon,
                foreground,
            );
            ctx.transform_layer_shapes(
                layer,
                egui::emath::TSTransform {
                    scaling: scale,
                    translation: origin.to_vec2()
                        + Vec2::new(
                            if slot == 0 { 8.0 } else { 64.0 },
                            if slot == 0 { 39.0 } else { 18.0 },
                        ),
                },
            );
        }
    }
    let y = 76.0 + UiIcon::ALL.len().div_ceil(6) as f32 * 88.0;
    for (index, (name, small, large)) in [
        (
            "Application / production · 32 px / 128 px",
            include_bytes!("../../assets/icons/tiptoptyp-32.png").as_slice(),
            include_bytes!("../../assets/icons/tiptoptyp-128.png").as_slice(),
        ),
        (
            "Application / development · 32 px / 128 px",
            include_bytes!("../../assets/icons/tiptoptyp-dev-32.png").as_slice(),
            include_bytes!("../../assets/icons/tiptoptyp-dev-128.png").as_slice(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let x = 24.0 + index as f32 * 440.0;
        text(egui::pos2(x, y), 13.0, name);
        for (offset, size, bytes) in [(0.0, 32, small), (64.0, 128, large)] {
            let id = egui::Id::new(("icon-audit-app", index, size));
            let cached = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id));
            let texture = cached.unwrap_or_else(|| {
                let image = image::load_from_memory(bytes)
                    .expect("application icon")
                    .into_rgba8();
                let texture = ctx.load_texture(
                    name,
                    egui::ColorImage::from_rgba_unmultiplied([size, size], image.as_raw()),
                    egui::TextureOptions::LINEAR,
                );
                ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
                texture
            });
            let rect =
                Rect::from_min_size(egui::pos2(x + offset, y + 24.0), Vec2::splat(size as f32));
            ui.painter().image(
                texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}
