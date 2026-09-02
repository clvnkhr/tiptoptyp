use std::path::Path;

use eframe::egui::{Color32, FontFamily, FontId, Stroke, TextFormat, text::LayoutJob};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

/// Syntax highlighting for non-Typst text files, backed by Syntect's bundled
/// Sublime grammars. Typst keeps using its own incremental parser.
pub struct GenericSyntaxHighlighter {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
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
            cached_source: String::new(),
            cached_extension: None,
            cached_dark_mode: true,
            cached_job: LayoutJob::default(),
            has_cache: false,
        }
    }
}

impl GenericSyntaxHighlighter {
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
        let theme_name = if dark_mode {
            "base16-ocean.dark"
        } else {
            "InspiredGitHub"
        };
        let theme = &self.themes.themes[theme_name];
        let mut highlighter = HighlightLines::new(syntax, theme);
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
                            font_id: FontId::new(15.0, FontFamily::Monospace),
                            color,
                            italics: style.font_style.contains(FontStyle::ITALIC),
                            ..Default::default()
                        };
                        if style.font_style.contains(FontStyle::UNDERLINE) {
                            format.underline = Stroke::new(1.0, color);
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
        font_id: FontId::new(15.0, FontFamily::Monospace),
        color: if dark_mode {
            Color32::from_rgb(214, 219, 230)
        } else {
            Color32::from_rgb(52, 58, 70)
        },
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
}
