#[path = "../src/sublime_theme.rs"]
mod sublime_theme;

use std::path::Path;

use sublime_theme::{ImportError, ThemeFormat, import_bytes, import_path};

const MODERN_DARK: &str = r##"
{
    // Sublime color schemes commonly permit comments and trailing commas.
    "name": "Fixture Night",
    "author": "tiptoptyp tests",
    "variables": {
        "ink": "#e8ecf2",
        "blue": "hsl(213, 100%, 65%)",
        "green": "#70c98f",
    },
    "globals": {
        "background": "#20242c",
        "foreground": "var(ink)",
        "caret": "var(blue)",
        "accent": "var(blue)",
        "line_highlight": "color(var(ink) alpha(0.06))",
        "selection": "color(var(blue) alpha(0.30))",
        "gutter_foreground": "#aeb7c5",
    },
    "rules": [
        { "scope": "comment", "foreground": "#aeb7c5", "font_style": "italic" },
        { "scope": "string", "foreground": "var(green)" },
        { "scope": "keyword, storage.type", "foreground": "var(blue)" },
    ],
}
"##;

const TEXTMATE_LIGHT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>name</key><string>Fixture Day</string>
  <key>author</key><string>tiptoptyp tests</string>
  <key>settings</key>
  <array>
    <dict>
      <key>settings</key>
      <dict>
        <key>background</key><string>#fafbfc</string>
        <key>foreground</key><string>#252a31</string>
        <key>caret</key><string>#005fcc</string>
        <key>selection</key><string>#b8d7ff</string>
      </dict>
    </dict>
    <dict>
      <key>name</key><string>Strings</string>
      <key>scope</key><string>string</string>
      <key>settings</key><dict><key>foreground</key><string>#16733b</string></dict>
    </dict>
  </array>
</dict>
</plist>
"##;

#[test]
fn imports_modern_color_scheme_and_derives_semantic_roles() {
    let imported = import_bytes(
        Path::new("Fixture.sublime-color-scheme"),
        MODERN_DARK.as_bytes(),
    )
    .expect("modern scheme should load");

    assert_eq!(imported.format, ThemeFormat::SublimeColorScheme);
    assert_eq!(imported.name.as_deref(), Some("Fixture Night"));
    assert_eq!(imported.author.as_deref(), Some("tiptoptyp tests"));
    assert!(imported.dark_mode);
    assert_eq!(imported.palette.editor_background.to_hex(), "#20242cff");
    assert_eq!(imported.palette.foreground.to_hex(), "#e8ecf2ff");
    assert_eq!(imported.palette.string.to_hex(), "#70c98fff");
    assert_eq!(imported.palette.current_line.a, 15);
    assert!(
        imported
            .palette
            .foreground
            .contrast_ratio(imported.palette.editor_background)
            >= 4.5
    );
    assert!(
        imported
            .palette
            .muted
            .contrast_ratio(imported.palette.editor_background)
            >= 4.5
    );
}

#[test]
fn imports_textmate_theme_through_syntect() {
    let imported = import_bytes(Path::new("Fixture.tmTheme"), TEXTMATE_LIGHT.as_bytes())
        .expect("TextMate theme should load");

    assert_eq!(imported.format, ThemeFormat::TextMate);
    assert_eq!(imported.name.as_deref(), Some("Fixture Day"));
    assert!(!imported.dark_mode);
    assert_eq!(imported.palette.editor_background.to_hex(), "#fafbfcff");
    assert_eq!(imported.palette.string.to_hex(), "#16733bff");
    assert_eq!(imported.syntect_theme.scopes.len(), 1);
}

#[test]
fn sparse_low_contrast_theme_gets_accessible_deterministic_fallbacks() {
    let source = br##"{
        "name": "Sparse",
        "globals": { "background": "#ffffff", "foreground": "#eeeeee" },
        "rules": [{ "scope": "comment", "foreground": "#f0f0f0" }]
    }"##;
    let imported = import_bytes(Path::new("Sparse.sublime-color-scheme"), source).unwrap();

    assert!(!imported.dark_mode);
    for color in [
        imported.palette.foreground,
        imported.palette.muted,
        imported.palette.comment,
        imported.palette.keyword,
        imported.palette.error,
    ] {
        assert!(
            color.contrast_ratio(imported.palette.editor_background) >= 4.5,
            "{} was not readable on {}",
            color.to_hex(),
            imported.palette.editor_background.to_hex()
        );
    }
    assert_ne!(imported.palette.error, imported.palette.foreground);
    assert_ne!(imported.palette.success, imported.palette.foreground);
    assert_ne!(imported.palette.keyword, imported.palette.foreground);
    assert_eq!(
        imported.palette,
        import_bytes(Path::new("again.sublime-color-scheme"), source)
            .unwrap()
            .palette
    );
}

#[test]
fn rejects_unknown_extensions_and_malformed_input_cleanly() {
    assert!(matches!(
        import_bytes(Path::new("theme.json"), b"{}"),
        Err(ImportError::UnsupportedFormat { .. })
    ));
    assert!(matches!(
        import_bytes(Path::new("theme.sublime-color-scheme"), b"{ nope"),
        Err(ImportError::Parse { .. })
    ));
}

#[test]
fn imports_a_theme_from_disk() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Fixture.sublime-color-scheme");
    std::fs::write(&path, MODERN_DARK).unwrap();

    let imported = import_path(&path).unwrap();

    assert_eq!(imported.name.as_deref(), Some("Fixture Night"));
}
