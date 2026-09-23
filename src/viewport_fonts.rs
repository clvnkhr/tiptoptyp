//! Keep immediate child uploads ordered with a parent's repeated layout pass.
use eframe::egui;

/// A new native surface must be created after a native appearance change has
/// reached AppKit. Existing children stay alive and retain their focus/geometry.
pub(crate) fn appearance_changed(context: &egui::Context) {
    let frame = context.cumulative_frame_nr_for(egui::ViewportId::ROOT);
    context.data_mut(|data| data.insert_temp(egui::Id::new("native-appearance-frame"), frame));
    context.request_repaint();
}
fn defer_new_child(changed: Option<u64>, frame: u64, exists: bool) -> bool {
    !exists && changed == Some(frame)
}
pub(crate) fn show_deferred(
    context: &egui::Context,
    id: egui::ViewportId,
    builder: egui::ViewportBuilder,
    body: impl Fn(&mut egui::Ui, egui::ViewportClass) + Send + Sync + 'static,
) {
    let changed =
        context.data(|data| data.get_temp::<u64>(egui::Id::new("native-appearance-frame")));
    let exists = context.input(|input| input.raw.viewports.contains_key(&id));
    if !context.embed_viewports()
        && defer_new_child(
            changed,
            context.cumulative_frame_nr_for(egui::ViewportId::ROOT),
            exists,
        )
    {
        context.request_repaint();
        return;
    }
    // Deferred painting has no nested parent/child texture-delta ordering to
    // repair, so it needs neither an atlas copy nor an extra upload here.
    context.show_viewport_deferred(id, creation_options(builder, exists), body);
}
pub(crate) fn show_immediate(
    context: &egui::Context,
    id: egui::ViewportId,
    builder: egui::ViewportBuilder,
    body: impl FnMut(&mut egui::Ui, egui::ViewportClass),
) {
    let changed =
        context.data(|data| data.get_temp::<u64>(egui::Id::new("native-appearance-frame")));
    let exists = context.input(|input| input.raw.viewports.contains_key(&id));
    if !context.embed_viewports()
        && defer_new_child(
            changed,
            context.cumulative_frame_nr_for(egui::ViewportId::ROOT),
            exists,
        )
    {
        context.request_repaint();
        return;
    }
    context.show_viewport_immediate(id, creation_options(builder, exists), body);
    if !context.embed_viewports() {
        after_immediate_viewport(context);
    }
}

/// `active` is a winit creation hint, not a live focus property. egui's
/// builder patch recreates the native window when it changes. Preserve the
/// surface across focus handoffs; callers use ViewportCommand::Focus for
/// explicit user actions on an existing window.
fn creation_options(mut builder: egui::ViewportBuilder, exists: bool) -> egui::ViewportBuilder {
    if exists {
        builder.active = None;
    }
    builder
}

fn after_immediate_viewport(context: &egui::Context) {
    // A cached empty galley is a cheap witness for the lifetime of egui's font
    // cache. Atlas recreation invalidates that cache (including on font, theme,
    // density, or atlas-capacity changes). Keep a witness per parent viewport.
    let marker = context.fonts_mut(|fonts| {
        fonts.layout_no_wrap(String::new(), egui::FontId::default(), egui::Color32::WHITE)
    });
    let id = egui::Id::new(("immediate-viewport-font-atlas", context.viewport_id()));
    let changed = context.data_mut(|data| {
        let unchanged = data
            .get_temp::<std::sync::Arc<egui::Galley>>(id)
            .is_some_and(|previous| std::sync::Arc::ptr_eq(&previous, &marker));
        data.insert_temp(id, marker);
        !unchanged
    });
    if context.current_pass_index() > 0 || changed {
        // egui accumulates a parent's texture deltas across layout passes, but
        // an immediate child paints before that parent output reaches Glow.
        // A full atlas from an earlier parent pass can then erase glyphs the
        // child just uploaded. Supersede it with the current atlas after a
        // cache change or repeated pass. Ordinary frames retain incremental
        // uploads and cached layouts.
        let atlas = context.fonts(|fonts| fonts.image());
        context.tex_manager().write().set(
            egui::TextureId::default(),
            egui::epaint::ImageDelta::full(atlas, egui::TextureOptions::LINEAR),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn focus_handoffs_never_recreate_existing_native_surfaces() {
        for initial in [false, true] {
            let mut native = creation_options(
                crate::window_policy::popup("test").with_active(initial),
                false,
            );
            assert_eq!(native.active, Some(initial));
            for focused in [true, false, true, false] {
                let (commands, recreate) = native.patch(creation_options(
                    crate::window_policy::popup("test").with_active(focused),
                    true,
                ));
                assert!(!recreate, "focus handoff must retain the native window");
                assert!(commands.is_empty(), "painting must not steal focus");
            }
        }
    }

    #[test]
    fn appearance_barrier_defers_only_new_children_for_one_frame() {
        assert!(defer_new_child(Some(4), 4, false));
        assert!(!defer_new_child(Some(4), 5, false));
        assert!(!defer_new_child(Some(4), 4, true));
        assert!(!defer_new_child(None, 4, false));
    }

    fn apply(image: &mut egui::ColorImage, mut delta: egui::TexturesDelta) {
        if let Some(updates) = delta.set.remove(&egui::TextureId::default()) {
            for update in updates {
                let egui::ImageData::Color(patch) = update.image;
                if let Some([x, y]) = update.pos {
                    for row in 0..patch.height() {
                        let start = (y + row) * image.width() + x;
                        image.pixels[start..start + patch.width()].copy_from_slice(
                            &patch.pixels[row * patch.width()..(row + 1) * patch.width()],
                        );
                    }
                } else {
                    *image = (*patch).clone();
                }
            }
        }
        delta.clear();
    }

    fn repeated_parent_pass(repair: bool) -> (egui::ColorImage, egui::ColorImage) {
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let uploaded = Rc::new(RefCell::new(egui::ColorImage::filled(
            [1, 1],
            egui::Color32::TRANSPARENT,
        )));
        context.set_theme(egui::Theme::Dark);
        let warmup = context.run_ui(egui::RawInput::default(), |_| {});
        apply(&mut uploaded.borrow_mut(), warmup.textures_delta);
        context.set_theme(egui::Theme::Light);
        let child_uploads = uploaded.clone();
        egui::Context::set_immediate_viewport_renderer(move |context, mut child| {
            let mut input = egui::RawInput {
                viewport_id: child.ids.this,
                ..Default::default()
            };
            input.viewports.insert(
                child.ids.this,
                egui::ViewportInfo {
                    parent: Some(child.ids.parent),
                    ..Default::default()
                },
            );
            let output = context.run_ui(input, |ui| (child.viewport_ui_cb)(ui));
            apply(&mut child_uploads.borrow_mut(), output.textures_delta);
        });
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Parent editor");
            if ui.ctx().current_pass_index() == 0 {
                ui.ctx().request_discard("Resolve parent layout first");
            } else {
                ui.ctx().show_viewport_immediate(
                    egui::ViewportId::from_hash_of("diff"),
                    egui::ViewportBuilder::default(),
                    |ui, _| {
                        ui.heading("New child glyphs: WXYZ 0123456789");
                    },
                );
                if repair {
                    after_immediate_viewport(ui.ctx());
                }
            }
        });
        assert_eq!(output.platform_output.num_completed_passes, 2);
        apply(&mut uploaded.borrow_mut(), output.textures_delta);
        let cpu = context.fonts(|fonts| fonts.image());
        let gpu = uploaded.borrow().clone();
        (cpu, gpu)
    }

    #[test]
    fn repeated_parent_pass_preserves_glyphs_uploaded_by_its_child() {
        let (cpu, gpu) = repeated_parent_pass(false);
        assert_ne!(cpu, gpu, "fixture must reproduce the upstream ordering bug");
        let (cpu, gpu) = repeated_parent_pass(true);
        assert_eq!(cpu, gpu);
    }

    #[test]
    fn normal_frames_do_not_upload_the_full_atlas_again() {
        let context = egui::Context::default();
        let first = context.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Cached editor text");
            after_immediate_viewport(ui.ctx());
        });
        let mut uploaded = egui::ColorImage::filled([1, 1], egui::Color32::TRANSPARENT);
        apply(&mut uploaded, first.textures_delta);
        let mut next = context.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Cached editor text");
            after_immediate_viewport(ui.ctx());
        });
        assert!(
            !next
                .textures_delta
                .set
                .contains_key(&egui::TextureId::default())
        );
        next.textures_delta.clear();
    }

    #[test]
    fn replacing_the_font_cache_resynchronizes_once_without_invalidating_layouts() {
        let context = egui::Context::default();
        let frame = || {
            context.run_ui(egui::RawInput::default(), |ui| {
                let layout = |fonts: &mut egui::epaint::text::FontsView<'_>| {
                    fonts.layout_no_wrap(
                        "Editor and diff text".into(),
                        egui::FontId::default(),
                        egui::Color32::WHITE,
                    )
                };
                let before = ui.fonts_mut(layout);
                after_immediate_viewport(ui.ctx());
                let after = ui.fonts_mut(layout);
                assert!(std::sync::Arc::ptr_eq(&before, &after));
            })
        };
        frame().textures_delta.clear();
        let mut fonts = egui::FontDefinitions::default();
        fonts.families.insert(
            egui::FontFamily::Name("new editor family".into()),
            fonts.families[&egui::FontFamily::Monospace].clone(),
        );
        context.set_fonts(fonts);
        let mut changed = frame();
        assert!(
            changed.textures_delta.set[&egui::TextureId::default()]
                .iter()
                .any(|delta| delta.pos.is_none())
        );
        changed.textures_delta.clear();
        let mut unchanged = frame();
        assert!(
            !unchanged
                .textures_delta
                .set
                .contains_key(&egui::TextureId::default())
        );
        unchanged.textures_delta.clear();
    }
}
