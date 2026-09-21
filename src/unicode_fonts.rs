//! Bundled missing-glyph coverage, registered once with the selected fonts.
//! Keep these behind the primary faces so ordinary text keeps its metrics.
use std::sync::Arc;

use eframe::egui::{FontData, FontDefinitions};

const FONTS: [(&str, &[u8]); 4] = [
    (
        "tiptoptyp-noto-symbols",
        include_bytes!("../assets/fonts/notosanssymbols/NotoSansSymbols[wght].ttf"),
    ),
    (
        "tiptoptyp-noto-math",
        include_bytes!("../assets/fonts/notosansmath/NotoSansMath-Regular.ttf"),
    ),
    (
        "tiptoptyp-noto-symbols2",
        include_bytes!("../assets/fonts/notosanssymbols2/NotoSansSymbols2-Regular.ttf"),
    ),
    (
        "tiptoptyp-noto-hebrew",
        include_bytes!("../assets/fonts/notosanshebrew/NotoSansHebrew[wdth,wght].ttf"),
    ),
];

pub(crate) const SYMBOL_EXAMPLES: [(&str, char); 14] = [
    ("aleph", 'א'),
    ("beth", 'ב'),
    ("daleth", 'ד'),
    ("lamed", 'ל'),
    ("nothing", '∅'),
    ("without", '∖'),
    ("forall", '∀'),
    ("complement", '∁'),
    ("left-tack", '⊰'),
    ("implies", '⟹'),
    ("fraktur-a", '𝔄'),
    ("script-a", '𝒜'),
    ("earth", '🜨'),
    ("wreath", '≀'),
];

pub(crate) fn definitions() -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    for (name, bytes) in FONTS {
        definitions
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
        for family in definitions.families.values_mut() {
            family.push(name.into());
        }
    }
    definitions
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Color32, FontFamily, FontId, epaint::text::Fonts};
    use skrifa::MetadataProvider;

    pub(crate) const SYMBOLS: &str = "א ב ד ל ∅ ∖ ≀ 🜨 ∀ ∁ ⊰ ⟹ 𝔄 𝒜 ⨳ ⨌";

    #[test]
    fn reported_and_related_symbols_have_bundled_glyphs_without_system_fonts() {
        let mut fonts = Fonts::new(Default::default(), definitions());
        let mut view = fonts.with_pixels_per_point(1.0);
        for family in [FontFamily::Monospace, FontFamily::Proportional] {
            let font = FontId::new(16.0, family);
            let missing = view.layout_no_wrap("\u{0378}".into(), font.clone(), Color32::WHITE);
            for character in SYMBOLS.chars().filter(|c| !c.is_whitespace()) {
                assert!(
                    FONTS.iter().any(|(_, bytes)| {
                        skrifa::FontRef::new(bytes)
                            .unwrap()
                            .charmap()
                            .map(character)
                            .is_some_and(|glyph| glyph.to_u32() != 0)
                    }),
                    "missing U+{:04X}",
                    character as u32
                );
                let rendered = view.layout_no_wrap(character.into(), font.clone(), Color32::WHITE);
                assert_ne!(
                    rendered.rows[0].glyphs[0].uv_rect, missing.rows[0].glyphs[0].uv_rect,
                    "U+{:04X} must render its glyph, not the replacement box",
                    character as u32
                );
            }
        }
    }

    #[test]
    fn fallbacks_preserve_primary_faces_and_latin_layout() {
        let original = FontDefinitions::default();
        let extended = definitions();
        for (family, names) in &original.families {
            assert!(extended.families[family].starts_with(names));
        }
        let mut original_fonts = Fonts::new(Default::default(), original);
        let mut extended_fonts = Fonts::new(Default::default(), extended);
        for family in [FontFamily::Monospace, FontFamily::Proportional] {
            let font = FontId::new(16.0, family);
            let text = "Normal editor text: #let x = (123, abc)";
            let before = original_fonts.with_pixels_per_point(1.0).layout_no_wrap(
                text.into(),
                font.clone(),
                Color32::WHITE,
            );
            let after = extended_fonts.with_pixels_per_point(1.0).layout_no_wrap(
                text.into(),
                font,
                Color32::WHITE,
            );
            assert_eq!(before.rect, after.rect);
            assert_eq!(before.rows[0].glyphs, after.rows[0].glyphs);
        }
    }
}
