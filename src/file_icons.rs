//! A bundled Nerd Font family, isolated from editor/terminal character metrics.
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, Rect};
use std::{path::Path, sync::Arc};

const FONT_NAME: &str = "tiptoptyp-file-icons";
fn font_bytes() -> &'static [u8] {
    crate::bundled_assets::font_bytes(crate::bundled_assets::AssetId::FileIcons)
}
const TEXT: char = '\u{e64e}';
const CONFIG: char = '\u{e615}';
const GIT: char = '\u{e65d}';

// Seti UI glyphs, plus Font Logos' Typst logo. Extensions are matched without
// allocation, filesystem access, or changing the font of the filename itself.
const EXTENSIONS: &[(&str, char)] = &[
    ("typ", '\u{f37f}'),
    ("tex", '\u{e69b}'),
    ("sty", '\u{e69b}'),
    ("cls", '\u{e69b}'),
    ("bib", '\u{e678}'),
    ("pdf", '\u{e67d}'),
    ("md", '\u{e609}'),
    ("markdown", '\u{e609}'),
    ("rs", '\u{e68b}'),
    ("py", '\u{e606}'),
    ("ipynb", '\u{e678}'),
    ("json", '\u{e60b}'),
    ("jsonc", '\u{e60b}'),
    ("toml", CONFIG),
    ("yaml", CONFIG),
    ("yml", CONFIG),
    ("ini", CONFIG),
    ("conf", CONFIG),
    ("lock", '\u{e672}'),
    ("png", '\u{e60d}'),
    ("jpg", '\u{e60d}'),
    ("jpeg", '\u{e60d}'),
    ("gif", '\u{e60d}'),
    ("webp", '\u{e60d}'),
    ("svg", '\u{e60d}'),
    ("bmp", '\u{e60d}'),
    ("ico", '\u{e60d}'),
    ("tif", '\u{e60d}'),
    ("tiff", '\u{e60d}'),
    ("mp3", '\u{e638}'),
    ("wav", '\u{e638}'),
    ("flac", '\u{e638}'),
    ("mp4", '\u{e69f}'),
    ("mov", '\u{e69f}'),
    ("webm", '\u{e69f}'),
    ("zip", '\u{e6aa}'),
    ("gz", '\u{e6aa}'),
    ("tar", '\u{e6aa}'),
    ("7z", '\u{e6aa}'),
    ("ttf", '\u{e659}'),
    ("otf", '\u{e659}'),
    ("woff", '\u{e659}'),
    ("woff2", '\u{e659}'),
    ("js", '\u{e60c}'),
    ("mjs", '\u{e60c}'),
    ("ts", '\u{e628}'),
    ("jsx", '\u{e625}'),
    ("tsx", '\u{e625}'),
    ("html", '\u{e60e}'),
    ("css", '\u{e614}'),
    ("scss", '\u{e603}'),
    ("vue", '\u{e6a0}'),
    ("sh", '\u{e691}'),
    ("bash", '\u{e691}'),
    ("zsh", '\u{e691}'),
    ("fish", '\u{e691}'),
    ("ps1", '\u{e683}'),
    ("go", '\u{e627}'),
    ("java", '\u{e66d}'),
    ("c", '\u{e649}'),
    ("h", '\u{e649}'),
    ("cpp", '\u{e646}'),
    ("hpp", '\u{e646}'),
    ("cs", '\u{e648}'),
    ("fs", '\u{e65a}'),
    ("lua", '\u{e620}'),
    ("r", '\u{e68a}'),
    ("rb", '\u{e605}'),
    ("pl", '\u{e67e}'),
    ("dart", '\u{e64c}'),
    ("csv", '\u{e64a}'),
    ("tsv", '\u{e64a}'),
    ("xml", '\u{e619}'),
];

pub(crate) fn for_path(path: Option<&Path>) -> char {
    let Some(path) = path else {
        return TEXT;
    };
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        if [".gitignore", ".gitattributes", ".gitmodules"]
            .iter()
            .any(|n| name.eq_ignore_ascii_case(n))
        {
            return GIT;
        }
        if name.eq_ignore_ascii_case("Dockerfile") {
            return '\u{e650}';
        }
        if name.eq_ignore_ascii_case("Makefile") {
            return '\u{e673}';
        }
        if name.eq_ignore_ascii_case(".editorconfig") {
            return '\u{e652}';
        }
    }
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return TEXT;
    };
    EXTENSIONS
        .iter()
        .find_map(|(ext, glyph)| extension.eq_ignore_ascii_case(ext).then_some(*glyph))
        .unwrap_or(TEXT)
}

fn family() -> FontFamily {
    static FAMILY: std::sync::OnceLock<FontFamily> = std::sync::OnceLock::new();
    FAMILY
        .get_or_init(|| FontFamily::Name(FONT_NAME.into()))
        .clone()
}

pub(crate) fn register(definitions: &mut FontDefinitions) {
    definitions.font_data.insert(
        FONT_NAME.into(),
        Arc::new(FontData::from_static(font_bytes())),
    );
    definitions
        .families
        .insert(family(), vec![FONT_NAME.into()]);
}

pub(crate) fn paint(painter: &egui::Painter, rect: Rect, glyph: char, color: Color32) -> bool {
    // Bare embedded egui components can be used without the app's font setup.
    // Their semantic tests still work; the app always registers the dedicated face.
    if !painter
        .ctx()
        .fonts(|fonts| fonts.definitions().families.contains_key(&family()))
    {
        return false;
    }
    let galley = painter.layout_no_wrap(glyph.into(), FontId::new(rect.height(), family()), color);
    // Nerd glyphs have heterogeneous sidebearings. Center their ink, not their
    // advance box, in the same fixed slot in the tree and the document tabs.
    let position = rect.center() - galley.mesh_bounds.center().to_vec2();
    painter.galley(position, galley, color);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::MetadataProvider;
    #[test]
    fn every_filetype_and_special_name_has_a_bundled_glyph() {
        let font = skrifa::FontRef::new(font_bytes()).unwrap();
        for path in EXTENSIONS
            .iter()
            .map(|(ext, _)| format!("sample.{ext}"))
            .chain(
                [
                    ".gitignore",
                    ".gitattributes",
                    ".gitmodules",
                    "Dockerfile",
                    "Makefile",
                    ".editorconfig",
                    "unknown",
                    "new.bin",
                ]
                .map(String::from),
            )
        {
            let glyph = for_path(Some(Path::new(&path)));
            assert!(
                font.charmap().map(glyph).is_some_and(|g| g.to_u32() != 0),
                "{path}: U+{:04X}",
                glyph as u32
            );
        }
    }
    #[test]
    fn classification_is_case_insensitive_and_uses_the_filename_only() {
        assert_eq!(for_path(Some(Path::new("a/b.TEX"))), '\u{e69b}');
        assert_eq!(for_path(Some(Path::new("a.typ/b.PDF"))), '\u{e67d}');
        assert_eq!(for_path(Some(Path::new("a.typ/b"))), TEXT);
        assert_eq!(for_path(Some(Path::new(".GITIGNORE"))), GIT);
        assert_eq!(for_path(None), TEXT);
        assert_ne!(
            for_path(Some(Path::new("a.typ"))),
            for_path(Some(Path::new("a.tex")))
        );
    }
    #[test]
    fn icon_font_is_isolated_from_text_and_terminal_metrics() {
        let before = FontDefinitions::default();
        let mut after = before.clone();
        register(&mut after);
        for (family, fonts) in before.families {
            assert_eq!(after.families[&family], fonts);
        }
        assert_eq!(after.families[&family()], [FONT_NAME]);
        assert!(
            font_bytes().len() < 64 * 1024,
            "keep the filetype subset small"
        );
    }
}
