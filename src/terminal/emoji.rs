//! Color emoji use the system font's bitmap glyphs, not egui's outline rasterizer.
//! Ghostty owns the grapheme and its column width; this adapter only paints it.
use eframe::egui;

#[derive(Default)]
pub(super) struct EmojiCache {
    #[cfg(target_os = "macos")]
    entries: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    #[cfg(target_os = "macos")]
    ppem: u16,
}

impl EmojiCache {
    pub(super) fn glyph(
        &mut self,
        context: &egui::Context,
        text: &str,
        columns: u8,
        pixels: f32,
    ) -> Option<&egui::TextureHandle> {
        #[cfg(target_os = "macos")]
        {
            if !color_candidate(text, columns) {
                return None;
            }
            let ppem = pixels.ceil().clamp(1.0, 128.0) as u16;
            if self.ppem != ppem {
                self.entries.clear();
                self.ppem = ppem;
            }
            if !self.entries.contains_key(text) {
                // Bound textures and negative lookups, including arbitrary CJK output.
                if self.entries.len() == 128 {
                    self.entries.clear();
                }
                let texture = system_font()
                    .and_then(|font| font.image(text, ppem))
                    .map(|image| {
                        context.load_texture("terminal-emoji", image, egui::TextureOptions::LINEAR)
                    });
                self.entries.insert(text.to_owned(), texture);
            }
            self.entries.get(text).and_then(Option::as_ref)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (context, text, columns, pixels);
            None
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn color_candidate(text: &str, columns: u8) -> bool {
    (columns == 2 || text.contains('\u{fe0f}')) && !text.is_ascii() && !text.contains('\u{fe0e}')
}

#[cfg(target_os = "macos")]
struct ColorFont {
    bytes: memmap2::Mmap,
    shaper: harfrust::ShaperData,
}

#[cfg(target_os = "macos")]
fn system_font() -> Option<&'static ColorFont> {
    static FONT: std::sync::OnceLock<Option<ColorFont>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        let file = std::fs::File::open("/System/Library/Fonts/Apple Color Emoji.ttc").ok()?;
        // SAFETY: this read-only font belongs to macOS's sealed system volume.
        // Mapping avoids copying the entire (~180 MiB) bitmap font into RAM.
        #[allow(unsafe_code)]
        let bytes = unsafe { memmap2::MmapOptions::new().map(&file).ok()? };
        let font = harfrust::FontRef::from_index(&bytes, 0).ok()?;
        let shaper = harfrust::ShaperData::new(&font);
        Some(ColorFont { bytes, shaper })
    })
    .as_ref()
}

#[cfg(target_os = "macos")]
impl ColorFont {
    fn image(&self, text: &str, ppem: u16) -> Option<egui::ColorImage> {
        use skrifa::{MetadataProvider, bitmap::BitmapData, instance::Size};
        let font = skrifa::FontRef::from_index(&self.bytes, 0).ok()?;
        // Don't shape CJK/wide text that the emoji face cannot cover.
        font.charmap().map(text.chars().next()?)?;
        let shape_font = harfrust::FontRef::from_index(&self.bytes, 0).ok()?;
        let mut buffer = harfrust::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.set_flags(harfrust::BufferFlags::REMOVE_DEFAULT_IGNORABLES);
        buffer.guess_segment_properties();
        let shaped = self
            .shaper
            .shaper(&shape_font)
            .build()
            .shape(buffer, harfrust::ShapeOptions::new());
        // A supported emoji sequence (ZWJ, skin tone, flag, keycap) is one glyph.
        let [glyph] = shaped.glyph_infos() else {
            return None;
        };
        if glyph.glyph_id == 0 {
            return None;
        }
        let bitmap = font.bitmap_strikes().glyph_for_size(
            Size::new(f32::from(ppem)),
            skrifa::GlyphId::new(glyph.glyph_id),
        )?;
        let BitmapData::Png(png) = bitmap.data else {
            return None;
        };
        let decoder =
            image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(256);
        limits.max_image_height = Some(256);
        let mut decoder = decoder;
        decoder.limits(limits);
        let image = decoder.decode().ok()?.into_rgba8();
        Some(egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_nerd_icons_and_explicit_text_presentation_keep_text_rendering() {
        for text in ["0", "*", "#", " ", "\u{f120}", "❤\u{fe0e}"] {
            assert!(!color_candidate(text, 1));
        }
        for text in ["😀", "🦀", "📦", "👩‍💻", "🇬🇧", "👍🏽", "1\u{fe0f}\u{20e3}"]
        {
            assert!(color_candidate(text, 2));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_emoji_sequences_render_colored_ink_and_cache_by_display_scale() {
        let started = std::time::Instant::now();
        let font = system_font().expect("macOS system emoji font");
        for text in ["😀", "🦀", "📦", "👩‍💻", "🇬🇧", "👍🏽", "1\u{fe0f}\u{20e3}"]
        {
            let image = font
                .image(text, 26)
                .unwrap_or_else(|| panic!("missing emoji: {text}"));
            assert!(
                image
                    .pixels
                    .iter()
                    .any(|pixel| pixel.a() > 100 && pixel.r().abs_diff(pixel.b()) > 30),
                "not colored: {text}"
            );
        }
        eprintln!(
            "system color emoji cold seven-grapheme probe: {:?}",
            started.elapsed()
        );
        let context = egui::Context::default();
        let mut cache = EmojiCache::default();
        assert!(cache.glyph(&context, "0", 1, 26.0).is_none());
        assert!(cache.entries.is_empty());
        let first = cache.glyph(&context, "😀", 2, 26.0).unwrap().id();
        for _ in 0..100 {
            assert_eq!(cache.glyph(&context, "😀", 2, 26.0).unwrap().id(), first);
        }
        assert_eq!(cache.entries.len(), 1);
        assert_ne!(cache.glyph(&context, "😀", 2, 52.0).unwrap().id(), first);
        for c in '\u{4e00}'..'\u{5000}' {
            assert!(cache.glyph(&context, &c.to_string(), 2, 52.0).is_none());
            assert!(cache.entries.len() <= 128);
        }
    }
}
