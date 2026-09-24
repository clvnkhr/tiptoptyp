use std::{
    path::Path,
    sync::{Arc, LazyLock},
};

use eframe::egui::{
    Color32, Stroke, TextFormat,
    text::{LayoutJob, LayoutSection},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Color as SyntectColor, FontStyle, HighlightState, Style, Theme, ThemeSet},
    parsing::{ParseState, SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};

use crate::theme::{self, METRICS};

/// Syntax highlighting for non-Typst text files, backed by Syntect's bundled
/// Sublime grammars. Typst keeps using its own incremental parser.
pub struct GenericSyntaxHighlighter {
    syntaxes: Arc<SyntaxSet>,
    themes: Arc<ThemeSet>,
    custom_theme: Option<Theme>,
    cached_source: String,
    cached_extension: Option<String>,
    cached_syntax: String,
    cached_lines: Vec<Arc<CachedLine>>,
    cached_dark_mode: bool,
    cached_job: LayoutJob,
    has_cache: bool,
    theme_revision: u64,
    #[cfg(test)]
    parsed_lines_last: usize,
}

struct CachedLine {
    text: String,
    job: LayoutJob,
    start: (HighlightState, ParseState),
    end: (HighlightState, ParseState),
}

impl Default for GenericSyntaxHighlighter {
    fn default() -> Self {
        // Hover rendering can construct a highlighter every frame. Decode the
        // bundled grammar/theme databases only once, shared across windows.
        static SYNTAXES: LazyLock<Arc<SyntaxSet>> =
            LazyLock::new(|| Arc::new(SyntaxSet::load_defaults_newlines()));
        static THEMES: LazyLock<Arc<ThemeSet>> =
            LazyLock::new(|| Arc::new(ThemeSet::load_defaults()));
        Self {
            syntaxes: SYNTAXES.clone(),
            themes: THEMES.clone(),
            custom_theme: None,
            cached_source: String::new(),
            cached_extension: None,
            cached_syntax: String::new(),
            cached_lines: Vec::new(),
            cached_dark_mode: true,
            cached_job: LayoutJob::default(),
            has_cache: false,
            theme_revision: 0,
            #[cfg(test)]
            parsed_lines_last: 0,
        }
    }
}

impl GenericSyntaxHighlighter {
    pub fn set_custom_theme(&mut self, theme: Option<Theme>) {
        self.custom_theme = theme;
        self.has_cache = false;
        self.theme_revision = self.theme_revision.wrapping_add(1);
    }

    pub(crate) fn theme_revision(&self) -> u64 {
        self.theme_revision
    }

    pub fn highlight(&mut self, source: &str, path: Option<&Path>, dark_mode: bool) -> LayoutJob {
        let extension = path
            .and_then(Path::extension)
            .and_then(|extension| extension.to_str());
        if self.has_cache
            && self.cached_source == source
            && extensions_match(self.cached_extension.as_deref(), extension)
            && self.cached_dark_mode == dark_mode
        {
            return self.cached_job.clone();
        }

        let normalized_extension = extension.map(str::to_ascii_lowercase);
        let syntaxes = Arc::clone(&self.syntaxes);
        let syntax = normalized_extension
            .as_deref()
            .and_then(|extension| syntaxes.find_syntax_by_extension(extension))
            .or_else(|| {
                source
                    .lines()
                    .next()
                    .and_then(|line| syntaxes.find_syntax_by_first_line(line))
            })
            .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
        let job = self.highlight_incrementally(source, syntax, dark_mode, extension);

        self.cached_source.clear();
        self.cached_source.push_str(source);
        self.cached_extension = normalized_extension;
        self.cached_dark_mode = dark_mode;
        self.cached_job = job.clone();
        self.has_cache = true;
        job
    }

    fn highlight_incrementally(
        &mut self,
        source: &str,
        syntax: &SyntaxReference,
        dark_mode: bool,
        extension: Option<&str>,
    ) -> LayoutJob {
        let lines = LinesWithEndings::from(source).collect::<Vec<_>>();
        let reuse = self.has_cache
            && self.cached_dark_mode == dark_mode
            && extensions_match(self.cached_extension.as_deref(), extension)
            && self.cached_syntax == syntax.name;
        let old = &self.cached_lines;
        let prefix = if reuse {
            old.iter()
                .zip(&lines)
                .take_while(|(cached, line)| cached.text == **line)
                .count()
        } else {
            0
        };
        let mut suffix = 0;
        if reuse {
            while suffix < old.len() - prefix
                && suffix < lines.len() - prefix
                && old[old.len() - suffix - 1].text == lines[lines.len() - suffix - 1]
            {
                suffix += 1;
            }
        }
        let old_suffix = old.len() - suffix;
        let new_suffix = lines.len() - suffix;
        let theme = self.selected_theme(dark_mode);
        let mut state = if prefix == 0 {
            HighlightLines::new(syntax, theme).state()
        } else {
            old[prefix - 1].end.clone()
        };
        let mut cached = Vec::with_capacity(lines.len());
        cached.extend(old[..prefix].iter().cloned());
        #[cfg(test)]
        let mut parsed_lines = 0;
        for (index, line) in lines.iter().enumerate().skip(prefix) {
            if index >= new_suffix {
                let corresponding = old_suffix + index - new_suffix;
                if state == old[corresponding].start {
                    cached.extend(old[corresponding..].iter().cloned());
                    break;
                }
            }
            let mut highlighter =
                HighlightLines::from_state(theme, state.0.clone(), state.1.clone());
            let mut job = LayoutJob::default();
            match highlighter.highlight_line(line, &self.syntaxes) {
                Ok(regions) => {
                    for (style, text) in regions {
                        job.append(text, 0.0, syntect_format(style, theme));
                    }
                }
                Err(_) => job.append(line, 0.0, plain_format(dark_mode)),
            }
            let end = highlighter.state();
            cached.push(Arc::new(CachedLine {
                text: (*line).to_owned(),
                job,
                start: state,
                end: end.clone(),
            }));
            state = end;
            #[cfg(test)]
            {
                parsed_lines += 1;
            }
        }
        let mut job = LayoutJob::default();
        job.text.reserve(source.len());
        for line in &cached {
            let offset = job.text.len();
            job.text.push_str(&line.job.text);
            job.sections
                .extend(line.job.sections.iter().map(|section| LayoutSection {
                    leading_space: section.leading_space,
                    byte_range: section.byte_range.start + offset..section.byte_range.end + offset,
                    format: section.format.clone(),
                }));
        }
        job.wrap.break_anywhere = false;
        debug_assert_eq!(job.text, source);
        self.cached_syntax.clone_from(&syntax.name);
        self.cached_lines = cached;
        #[cfg(test)]
        {
            self.parsed_lines_last = parsed_lines;
        }
        job
    }

    /// Highlight a fenced-code payload using a GitHub-style language token
    /// such as `rs`, `tex`, or `python`. Syntect provides this lookup expressly
    /// for language-tagged code blocks.
    pub(crate) fn highlight_token(
        &self,
        source: &str,
        token: &str,
        dark_mode: bool,
    ) -> Option<LayoutJob> {
        let syntax = self.syntaxes.find_syntax_by_token(token.trim())?;
        Some(self.highlight_with_syntax(source, syntax, dark_mode))
    }

    fn selected_theme(&self, dark_mode: bool) -> &Theme {
        self.custom_theme
            .as_ref()
            .unwrap_or_else(|| &self.themes.themes[theme::generic_syntax_theme_name(dark_mode)])
    }

    fn highlight_with_syntax(
        &self,
        source: &str,
        syntax: &SyntaxReference,
        dark_mode: bool,
    ) -> LayoutJob {
        let selected_theme = self.selected_theme(dark_mode);
        let mut highlighter = HighlightLines::new(syntax, selected_theme);
        let mut job = LayoutJob::default();

        for line in LinesWithEndings::from(source) {
            match highlighter.highlight_line(line, &self.syntaxes) {
                Ok(regions) => {
                    for (style, text) in regions {
                        job.append(text, 0.0, syntect_format(style, selected_theme));
                    }
                }
                Err(_) => job.append(line, 0.0, plain_format(dark_mode)),
            }
        }
        job.wrap.break_anywhere = false;
        debug_assert_eq!(job.text, source);
        job
    }
}

fn extensions_match(cached: Option<&str>, current: Option<&str>) -> bool {
    match (cached, current) {
        (None, None) => true,
        (Some(cached), Some(current)) => cached.eq_ignore_ascii_case(current),
        _ => false,
    }
}

fn syntect_format(style: Style, theme: &Theme) -> TextFormat {
    let color = syntect_color(style.foreground);
    // Syntect resolves the theme's global editor background into every style.
    // Keep that global color transparent here so cursor-line and diagnostic
    // tints painted behind glyphs remain visible; a differing scope-specific
    // background is retained.
    let background = if theme.settings.background == Some(style.background) {
        Color32::TRANSPARENT
    } else {
        syntect_color(style.background)
    };
    TextFormat {
        font_id: theme::editor_font_with_weight(if style.font_style.contains(FontStyle::BOLD) {
            theme::FONT_WEIGHT_BOLD
        } else {
            theme::FONT_WEIGHT_NORMAL
        }),
        color,
        background,
        italics: style.font_style.contains(FontStyle::ITALIC),
        underline: if style.font_style.contains(FontStyle::UNDERLINE) {
            Stroke::new(METRICS.syntax.link_underline_width, color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    }
}

fn syntect_color(color: SyntectColor) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn plain_format(dark_mode: bool) -> TextFormat {
    TextFormat {
        font_id: theme::editor_font(),
        color: theme::default_syntax_palette(dark_mode).plain,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "opt-in optimized TeX edit highlighting probe"]
    fn tex_edit_cost_probe() {
        use std::time::Instant;
        let mut highlighter = GenericSyntaxHighlighter::default();
        let mut source = (0..5_000)
            .map(|n| format!("\\section{{Section {n}}} Here is a paragraph with \\emph{{highlighted text}} and $x_n^2$.\n"))
            .collect::<String>();
        let path = Path::new("paper.tex");
        highlighter.highlight(&source, Some(path), false);
        let marker = source.find("Section 2500").unwrap();
        let started = Instant::now();
        for n in 0..25 {
            source.replace_range(marker..marker + 1, if n % 2 == 0 { "s" } else { "S" });
            let job = highlighter.highlight(&source, Some(path), false);
            assert_eq!(job.text, source);
        }
        println!(
            "fixture_bytes={} edits=25 total_ms={:.3} per_edit_ms={:.3}",
            source.len(),
            started.elapsed().as_secs_f64() * 1_000.0,
            started.elapsed().as_secs_f64() * 1_000.0 / 25.0
        );
    }

    #[test]
    fn extension_case_changes_preserve_highlighting() {
        let mut highlighter = GenericSyntaxHighlighter::default();
        let source = "fn main() { let value = 42; }";
        let lower = highlighter.highlight(source, Some(Path::new("main.rs")), true);
        let upper = highlighter.highlight(source, Some(Path::new("main.RS")), true);
        assert_eq!(lower, upper);
        let unknown = highlighter.highlight(source, Some(Path::new("main.unknown")), true);
        assert_ne!(lower.sections, unknown.sections);
        let no_extension = highlighter.highlight(source, Some(Path::new("main")), true);
        assert_eq!(unknown, no_extension);
    }

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
    fn tex_edits_reparse_only_until_the_syntax_state_rejoins() {
        let source = (0..1_000)
            .map(|n| format!("\\section{{Heading {n}}}\nText with \\emph{{color}} and $x^2$.\n"))
            .collect::<String>();
        let path = Some(Path::new("main.tex"));
        let mut cached = GenericSyntaxHighlighter::default();
        cached.highlight(&source, path, false);
        for needle in ["Heading 1", "Heading 500", "Heading 999"] {
            let mut edited = source.clone();
            let at = edited.find(needle).unwrap();
            edited.replace_range(at..at + 1, "h");
            let result = cached.highlight(&edited, path, false);
            let mut fresh = GenericSyntaxHighlighter::default();
            let expected = fresh.highlight(&edited, path, false);
            assert_eq!(result, expected, "wrong colors after editing {needle}");
            assert!(
                cached.parsed_lines_last <= 2,
                "unaffected TeX lines were reparsed"
            );
            cached.highlight(&source, path, false);
        }
    }

    #[test]
    fn multiline_tex_state_reparses_until_a_safe_boundary() {
        let source = "before\n\\begin{verbatim}\nold\n\\end{verbatim}\nafter\n";
        let path = Some(Path::new("main.tex"));
        let mut cached = GenericSyntaxHighlighter::default();
        cached.highlight(source, path, true);
        let edited = source.replace("\\end{verbatim}", "\\end{other}");
        let result = cached.highlight(&edited, path, true);
        let mut fresh = GenericSyntaxHighlighter::default();
        assert_eq!(result, fresh.highlight(&edited, path, true));
        assert!(cached.parsed_lines_last >= 2);
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
                    .all(|section| section.format.font_id == theme::editor_font()
                        || section.format.font_id
                            == theme::editor_font_with_weight(theme::FONT_WEIGHT_BOLD))
            );
            assert_eq!(plain_format(dark_mode).font_id, theme::editor_font());
        }
    }

    #[test]
    fn syntect_formats_preserve_scope_background_and_decorations() {
        let editor_background = SyntectColor {
            r: 10,
            g: 20,
            b: 30,
            a: 255,
        };
        let scope_background = SyntectColor {
            r: 40,
            g: 50,
            b: 60,
            a: 180,
        };
        let foreground = SyntectColor {
            r: 210,
            g: 220,
            b: 230,
            a: 255,
        };
        let mut custom_theme = Theme::default();
        custom_theme.settings.background = Some(editor_background);
        let format = syntect_format(
            Style {
                foreground,
                background: scope_background,
                font_style: FontStyle::BOLD | FontStyle::ITALIC | FontStyle::UNDERLINE,
            },
            &custom_theme,
        );

        assert_eq!(format.color, syntect_color(foreground));
        assert_eq!(format.background, syntect_color(scope_background));
        assert_eq!(
            format.font_id,
            theme::editor_font_with_weight(theme::FONT_WEIGHT_BOLD)
        );
        assert!(format.italics);
        assert_ne!(format.underline, Stroke::NONE);

        let inherited_background = syntect_format(
            Style {
                foreground,
                background: editor_background,
                font_style: FontStyle::empty(),
            },
            &custom_theme,
        );
        assert_eq!(inherited_background.background, Color32::TRANSPARENT);
    }

    #[test]
    fn language_tokens_resolve_fenced_code_grammars() {
        let highlighter = GenericSyntaxHighlighter::default();
        let tex = highlighter
            .highlight_token("\\textbf{hello}\n", "tex", true)
            .expect("the bundled syntax set should recognize tex");
        assert_eq!(tex.text, "\\textbf{hello}\n");
        assert!(
            highlighter
                .highlight_token("text", "not-a-language", true)
                .is_none()
        );
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
