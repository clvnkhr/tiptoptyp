//! Preview-only color transforms; never rewrite source or rebuild the document.
use super::*;
pub(super) fn recolor(rgba: &mut [u8], background: Color32, foreground: Color32) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        for channel in 0..3 {
            let value = u32::from(pixel[channel]);
            pixel[channel] = ((u32::from(foreground[channel]) * (255 - value)
                + u32::from(background[channel]) * value
                + 127)
                / 255) as u8;
        }
    }
}
impl EditorApp {
    pub(super) fn preview_palette(
        &self,
        context: &egui::Context,
        dark: bool,
    ) -> (Color32, Color32) {
        let visuals = &context.style_of(context.theme()).visuals;
        let (bg, fg) = if dark {
            (Color32::BLACK, Color32::WHITE)
        } else {
            (Color32::WHITE, Color32::BLACK)
        };
        let (comfy_bg, comfy_fg) = if dark == visuals.dark_mode {
            (visuals.panel_fill, visuals.text_color())
        } else {
            (visuals.text_color(), visuals.panel_fill)
        };
        (
            if self.settings.comfy_background {
                comfy_bg
            } else {
                bg
            },
            if self.settings.comfy_text {
                comfy_fg
            } else {
                fg
            },
        )
    }

    pub(super) fn toggle_comfy(&mut self, context: &egui::Context) {
        let mut edited = self.settings.clone();
        let enabled = !(edited.comfy_background && edited.comfy_text);
        edited.comfy_background = enabled;
        edited.comfy_text = enabled;
        self.queue_settings(edited, context);
        context.request_repaint();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_maps_black_and_white_exactly_and_preserves_alpha() {
        let bg = Color32::from_rgb(32, 45, 60);
        let fg = Color32::from_rgb(190, 200, 220);
        let mut rgba = [0, 0, 0, 255, 255, 255, 255, 137];
        recolor(&mut rgba, bg, fg);
        assert_eq!(rgba, [190, 200, 220, 255, 32, 45, 60, 137]);
    }
    #[test]
    fn comfy_background_and_text_are_independent() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        for dark in [false, true] {
            app.settings.comfy_background = false;
            app.settings.comfy_text = false;
            let plain = app.preview_palette(&context, dark);
            app.settings.comfy_background = true;
            let bg = app.preview_palette(&context, dark);
            assert_eq!(bg.1, plain.1);
            app.settings.comfy_text = true;
            let both = app.preview_palette(&context, dark);
            assert_eq!(both.0, bg.0);
            app.settings.comfy_background = false;
            let text = app.preview_palette(&context, dark);
            assert_eq!(text.0, plain.0);
            assert_eq!(text.1, both.1);
        }
    }
    #[test]
    fn comfy_changes_neither_document_nor_build_revision() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let before = app.document().snapshot();
        app.toggle_comfy(&context);
        assert!(app.pending_settings.as_ref().unwrap().comfy_background);
        assert!(app.pending_settings.as_ref().unwrap().comfy_text);
        assert_eq!(app.document().source().as_str(), before.source());
        assert_eq!(app.document().key(), before.key());
    }
}
