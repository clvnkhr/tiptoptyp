use std::path::Path;

use eframe::egui::{Color32, Stroke, TextFormat, text::LayoutJob};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

use crate::theme::{self, METRICS};

/// Syntax highlighting for non-Typst text files, backed by Syntect's bundled
/// Sublime grammars. Typst keeps using its own incremental parser.
pub struct GenericSyntaxHighlighter {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
    custom_theme: Option<Theme>,
    cached_source: String,
    cached_extension: Option<String>,
    cached_dark_mode: bool,
    cached_job: LayoutJob,
    has_cache: bool,
}

impl Default for GenericSyntaxHighlighter {
    fn default() -> Self {
        Self {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            themes: ThemeSet::load_defaults(),
            custom_theme: None,
            cached_source: String::new(),
            cached_extension: None,
            cached_dark_mode: true,
            cached_job: LayoutJob::default(),
            has_cache: false,
        }
    }
}

impl GenericSyntaxHighlighter {
    pub fn set_custom_theme(&mut self, theme: Option<Theme>) {
        self.custom_theme = theme;
        self.has_cache = false;
    }

    pub fn highlight(&mut self, source: &str, path: Option<&Path>, dark_mode: bool) -> LayoutJob {
        let extension = path
            .and_then(Path::extension)
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        if self.has_cache
            && self.cached_source == source
            && self.cached_extension == extension
            && self.cached_dark_mode == dark_mode
        {
            return self.cached_job.clone();
        }

        let syntax = extension
            .as_deref()
            .and_then(|extension| self.syntaxes.find_syntax_by_extension(extension))
            .or_else(|| {
                source
                    .lines()
                    .next()
                    .and_then(|line| self.syntaxes.find_syntax_by_first_line(line))
            })
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        let selected_theme = self
            .custom_theme
            .as_ref()
            .unwrap_or_else(|| &self.themes.themes[theme::generic_syntax_theme_name(dark_mode)]);
        let mut highlighter = HighlightLines::new(syntax, selected_theme);
        let mut job = LayoutJob::default();

        for line in LinesWithEndings::from(source) {
            match highlighter.highlight_line(line, &self.syntaxes) {
                Ok(regions) => {
                    for (style, text) in regions {
                        let color = Color32::from_rgb(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                        );
                        let mut format = TextFormat {
                            font_id: theme::editor_font(),
                            color,
                            italics: style.font_style.contains(FontStyle::ITALIC),
                            ..Default::default()
                        };
                        if style.font_style.contains(FontStyle::UNDERLINE) {
                            format.underline =
                                Stroke::new(METRICS.syntax.link_underline_width, color);
                        }
                        job.append(text, 0.0, format);
                    }
                }
                Err(_) => job.append(line, 0.0, plain_format(dark_mode)),
            }
        }
        job.wrap.break_anywhere = false;
        debug_assert_eq!(job.text, source);

        self.cached_source.clear();
        self.cached_source.push_str(source);
        self.cached_extension = extension;
        self.cached_dark_mode = dark_mode;
        self.cached_job = job.clone();
        self.has_cache = true;
        job
    }
}

fn plain_format(dark_mode: bool) -> TextFormat {
    TextFormat {
        font_id: theme::editor_font(),
        color: theme::syntax_palette(dark_mode).plain,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlighted_text_preserves_editor_byte_mapping() {
        let mut highlighter = GenericSyntaxHighlighter::default();
        for (path, source) in [
            ("main.rs", "fn main() { println!(\"hello\"); }\n"),
            ("data.json", "{\"answer\": 42}\n"),
            ("README", "plain UTF-8 — 🙂\n"),
        ] {
            let job = highlighter.highlight(source, Some(Path::new(path)), true);
            assert_eq!(job.text, source);
        }
    }

    #[test]
    fn cache_is_invalidated_by_extension_and_theme() {
        let mut highlighter = GenericSyntaxHighlighter::default();
        let source = "let value = 1;\n";
        let rust = highlighter.highlight(source, Some(Path::new("main.rs")), true);
        let text = highlighter.highlight(source, Some(Path::new("notes.txt")), true);
        let light = highlighter.highlight(source, Some(Path::new("notes.txt")), false);
        assert_eq!(rust.text, source);
        assert_eq!(text.text, source);
        assert_eq!(light.text, source);
        assert_ne!(
            text.sections[0].format.color,
            light.sections[0].format.color
        );
    }

    #[test]
    fn generic_formats_use_shared_theme_and_font_roles() {
        let mut highlighter = GenericSyntaxHighlighter::default();
        let source = "let value = 1;\n";

        for dark_mode in [false, true] {
            assert!(
                highlighter
                    .themes
                    .themes
                    .contains_key(theme::generic_syntax_theme_name(dark_mode))
            );
            let job = highlighter.highlight(source, Some(Path::new("main.rs")), dark_mode);
            assert!(
                job.sections
                    .iter()
                    .all(|section| section.format.font_id == theme::editor_font())
            );
            assert_eq!(plain_format(dark_mode).font_id, theme::editor_font());
        }
    }

    #[test]
    fn a_custom_sublime_theme_replaces_the_bundled_syntax_theme() {
        let mut highlighter = GenericSyntaxHighlighter::default();
        let mut custom = Theme::default();
        custom.settings.foreground = Some(syntect::highlighting::Color {
            r: 12,
            g: 34,
            b: 56,
            a: 255,
        });
        highlighter.set_custom_theme(Some(custom));
        let job = highlighter.highlight("plain text\n", Some(Path::new("notes.txt")), false);
        assert_eq!(job.sections[0].format.color, Color32::from_rgb(12, 34, 56));
    }
}
