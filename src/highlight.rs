use eframe::egui::{
    Color32,
    text::{ByteIndex, LayoutJob, LayoutSection},
};
#[cfg(test)]
use eframe::egui::{Stroke, TextFormat};
use typst_syntax::{LinkedNode, Source, SyntaxKind, Tag};

use crate::{
    generic_highlight::GenericSyntaxHighlighter, syntax_theme::ResolvedTypstStyles, theme,
};

/// Syntax highlighting backed by Typst's own error-tolerant parser.
///
/// Keeping a [`Source`] alive lets `Source::replace` incrementally reparse the
/// smallest changed region instead of rebuilding the syntax tree after every
/// keystroke. The completed layout job is cached as well because egui can ask
/// its layouter for the same text more than once in a frame.
pub struct SyntaxHighlighter {
    parsed_source: Source,
    cached_dark_mode: bool,
    cached_job: LayoutJob,
    has_cache: bool,
    cached_syntect_revision: u64,
    cached_code_mode: bool,
    styles: ResolvedTypstStyles,
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self {
            parsed_source: Source::detached(String::new()),
            cached_dark_mode: true,
            cached_job: LayoutJob::default(),
            has_cache: false,
            cached_syntect_revision: 0,
            cached_code_mode: false,
            styles: ResolvedTypstStyles::default(),
        }
    }
}

impl SyntaxHighlighter {
    pub(crate) fn set_styles(&mut self, styles: ResolvedTypstStyles) {
        if self.styles != styles {
            self.styles = styles;
            self.has_cache = false;
        }
    }

    pub fn highlight(
        &mut self,
        source: &str,
        dark_mode: bool,
        syntect: &GenericSyntaxHighlighter,
    ) -> LayoutJob {
        self.highlight_mode(source, dark_mode, syntect, false)
    }

    /// Highlight a Typst snippet as code, matching the editor's `#{...}` mode.
    ///
    /// Typst parses bare fenced payloads as markup, which makes words such as
    /// `let` look like ordinary prose. Wrapping the payload in a synthetic
    /// code-mode expression gives the parser the same context as the editor;
    /// the synthetic delimiters are removed again before returning the job.
    pub(crate) fn highlight_code(
        &mut self,
        source: &str,
        dark_mode: bool,
        syntect: &GenericSyntaxHighlighter,
    ) -> LayoutJob {
        self.highlight_mode(source, dark_mode, syntect, true)
    }

    fn highlight_mode(
        &mut self,
        source: &str,
        dark_mode: bool,
        syntect: &GenericSyntaxHighlighter,
        code_mode: bool,
    ) -> LayoutJob {
        let parse_source = if code_mode {
            format!("#{{{source}}}")
        } else {
            source.to_owned()
        };
        let source_changed = self.parsed_source.text() != parse_source;
        let syntect_revision = syntect.theme_revision();
        if self.has_cache
            && !source_changed
            && self.cached_dark_mode == dark_mode
            && self.cached_syntect_revision == syntect_revision
            && self.cached_code_mode == code_mode
        {
            return self.cached_job.clone();
        }

        if source_changed {
            // `replace` computes the common prefix/suffix and reparses only the
            // changed syntax-tree region. It is the incremental API supplied by
            // Typst itself.
            self.parsed_source.replace(&parse_source);
        }

        let mut job = LayoutJob::default();
        let root = LinkedNode::new(self.parsed_source.root());
        append_node(
            &mut job,
            &root,
            None,
            dark_mode,
            &parse_source,
            &self.styles,
            syntect,
        );
        if code_mode {
            trim_layout_job(&mut job, 2, 1);
        }
        job.wrap.break_anywhere = false;

        // TextEdit requires the galley's byte positions to map exactly back to
        // the editable string. This catches accidental omission or injection if
        // Typst's syntax-tree representation changes in the future.
        debug_assert_eq!(job.text, source);

        self.cached_dark_mode = dark_mode;
        self.cached_syntect_revision = syntect_revision;
        self.cached_code_mode = code_mode;
        self.cached_job = job.clone();
        self.has_cache = true;
        job
    }
}

fn trim_layout_job(job: &mut LayoutJob, prefix_bytes: usize, suffix_bytes: usize) {
    let end = job.text.len().saturating_sub(suffix_bytes);
    let start = prefix_bytes.min(end);
    let text = job.text[start..end].to_owned();
    let sections = job
        .sections
        .iter()
        .filter_map(|section| {
            let section_start = section.byte_range.start.0.max(start);
            let section_end = section.byte_range.end.0.min(end);
            (section_start < section_end).then(|| LayoutSection {
                leading_space: if section_start == section.byte_range.start.0 {
                    section.leading_space
                } else {
                    0.0
                },
                byte_range: ByteIndex(section_start - start)..ByteIndex(section_end - start),
                format: section.format.clone(),
            })
        })
        .collect();
    job.text = text;
    job.sections = sections;
}

/// Walk leaves in source order. A tag on the closest node wins; this preserves
/// structural tags such as headings while allowing nested constructs (strong,
/// raw, interpolation, errors, and so on) to override them.
fn append_node(
    job: &mut LayoutJob,
    node: &LinkedNode<'_>,
    inherited: Option<Tag>,
    dark: bool,
    source: &str,
    styles: &ResolvedTypstStyles,
    syntect: &GenericSyntaxHighlighter,
) {
    let tag = typst_syntax::highlight(node).or(inherited);
    let text = node.leaf_text();

    if text.is_empty() {
        if node.kind() == SyntaxKind::Raw
            && append_tagged_raw(job, node, dark, source, styles, syntect)
        {
            return;
        }
        for child in node.children() {
            append_node(job, &child, tag, dark, source, styles, syntect);
        }
    } else {
        let mut format = styles.format(tag);
        if matches!(tag, Some(Tag::String))
            && let Some(color) = parse_hex_color_string(text)
        {
            let swatch = composite_over_editor(color, dark);
            format.background = swatch;
            format.color = contrast_text(swatch);
        }
        job.append(text, 0.0, format);
    }
}

fn append_tagged_raw(
    job: &mut LayoutJob,
    node: &LinkedNode<'_>,
    dark: bool,
    source: &str,
    styles: &ResolvedTypstStyles,
    syntect: &GenericSyntaxHighlighter,
) -> bool {
    let children = node.children().collect::<Vec<_>>();
    let Some(language) = children
        .iter()
        .find(|child| child.kind() == SyntaxKind::RawLang)
    else {
        return false;
    };
    let Some(closing) = children
        .last()
        .filter(|child| child.kind() == SyntaxKind::RawDelim)
    else {
        return false;
    };
    let prefix_range = node.offset()..language.range().end;
    let content_range = language.range().end..closing.offset();
    let suffix_range = closing.offset()..node.range().end;
    let (Some(prefix), Some(content), Some(suffix)) = (
        source.get(prefix_range),
        source.get(content_range),
        source.get(suffix_range),
    ) else {
        return false;
    };
    let Some(highlighted) = syntect.highlight_token(content, language.leaf_text(), dark) else {
        return false;
    };

    let raw = styles.format(Some(Tag::Raw));
    job.append(prefix, 0.0, raw.clone());
    append_layout_job(job, &highlighted);
    job.append(suffix, 0.0, raw);
    true
}

fn append_layout_job(target: &mut LayoutJob, source: &LayoutJob) {
    for section in &source.sections {
        let Some(text) = source
            .text
            .get(section.byte_range.start.0..section.byte_range.end.0)
        else {
            continue;
        };
        target.append(text, section.leading_space, section.format.clone());
    }
}

#[cfg(test)]
fn format_for(tag: Option<Tag>, dark: bool) -> TextFormat {
    ResolvedTypstStyles::resolve(theme::syntax_palette(dark), None, &Default::default()).format(tag)
}

fn parse_hex_color_string(text: &str) -> Option<Color32> {
    let hex = text
        .strip_prefix('"')?
        .strip_suffix('"')?
        .strip_prefix('#')?;
    let expand = |digit: u8| (digit << 4) | digit;
    let nibble = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    let byte = |pair: &[u8]| Some((nibble(pair[0])? << 4) | nibble(pair[1])?);
    let bytes = hex.as_bytes();

    let (red, green, blue, alpha) = match bytes.len() {
        3 => (
            expand(nibble(bytes[0])?),
            expand(nibble(bytes[1])?),
            expand(nibble(bytes[2])?),
            255,
        ),
        4 => (
            expand(nibble(bytes[0])?),
            expand(nibble(bytes[1])?),
            expand(nibble(bytes[2])?),
            expand(nibble(bytes[3])?),
        ),
        6 => (
            byte(&bytes[0..2])?,
            byte(&bytes[2..4])?,
            byte(&bytes[4..6])?,
            255,
        ),
        8 => (
            byte(&bytes[0..2])?,
            byte(&bytes[2..4])?,
            byte(&bytes[4..6])?,
            byte(&bytes[6..8])?,
        ),
        _ => return None,
    };
    Some(Color32::from_rgba_unmultiplied(red, green, blue, alpha))
}

fn composite_over_editor(color: Color32, dark: bool) -> Color32 {
    if color.a() == 255 {
        return color;
    }
    let base = theme::syntax_palette(dark).editor_background;
    let alpha = color.a() as u16;
    let blend = |foreground: u8, background: u8| {
        ((foreground as u16 * alpha + background as u16 * (255 - alpha) + 127) / 255) as u8
    };
    Color32::from_rgb(
        blend(color.r(), base.r()),
        blend(color.g(), base.g()),
        blend(color.b(), base.b()),
    )
}

fn contrast_text(background: Color32) -> Color32 {
    let linear = |channel: u8| {
        let channel = channel as f32 / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(background.r())
        + 0.7152 * linear(background.g())
        + 0.0722 * linear(background.b());
    let white_contrast = 1.05 / (luminance + 0.05);
    let black_contrast = (luminance + 0.05) / 0.05;
    if white_contrast >= black_contrast {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sublime_theme::Rgba,
        syntax_theme::{TypstStyleOverrides, TypstSyntaxRole},
    };

    fn highlight(source: &str, dark: bool) -> LayoutJob {
        let mut highlighter = SyntaxHighlighter::default();
        let syntect = GenericSyntaxHighlighter::default();
        highlighter.highlight(source, dark, &syntect)
    }

    fn format_at(job: &LayoutJob, byte: usize) -> &TextFormat {
        &job.sections
            .iter()
            .find(|section| section.byte_range.start.0 <= byte && byte < section.byte_range.end.0)
            .expect("the source byte should be covered by a layout section")
            .format
    }

    fn assert_exact_mapping(job: &LayoutJob, source: &str) {
        assert_eq!(job.text, source);

        let mut next_byte = 0;
        for section in &job.sections {
            assert_eq!(section.byte_range.start.0, next_byte);
            assert!(section.byte_range.end > section.byte_range.start);
            assert!(source.is_char_boundary(section.byte_range.start.0));
            assert!(source.is_char_boundary(section.byte_range.end.0));
            next_byte = section.byte_range.end.0;
        }
        assert_eq!(next_byte, source.len());
    }

    #[test]
    fn markup_words_that_resemble_keywords_stay_plain() {
        let source = "The PDF preview updates as you type.";
        let job = highlight(source, true);
        let as_byte = source.find(" as ").unwrap() + 1;

        assert_exact_mapping(&job, source);
        assert_eq!(format_at(&job, as_byte).color, format_for(None, true).color);
        assert_ne!(
            format_at(&job, as_byte).color,
            format_for(Some(Tag::Keyword), true).color
        );
    }

    #[test]
    fn code_keywords_use_typsts_keyword_tag() {
        let source = "#import \"library.typ\" as library";
        let job = highlight(source, true);
        let as_byte = source.find(" as ").unwrap() + 1;

        assert_exact_mapping(&job, source);
        assert_eq!(
            format_at(&job, as_byte).color,
            format_for(Some(Tag::Keyword), true).color
        );
    }

    #[test]
    fn code_fenced_typst_payload_uses_code_mode_without_delimiters() {
        let source = "let value = 42\ntext(value)";
        let mut highlighter = SyntaxHighlighter::default();
        let syntect = GenericSyntaxHighlighter::default();
        let job = highlighter.highlight_code(source, true, &syntect);

        assert_exact_mapping(&job, source);
        assert_eq!(
            format_at(&job, source.find("let").unwrap()).color,
            format_for(Some(Tag::Keyword), true).color
        );
        assert_eq!(
            format_at(&job, source.find("42").unwrap()).color,
            format_for(Some(Tag::Number), true).color
        );
        assert_eq!(
            format_at(&job, source.find("text").unwrap()).color,
            format_for(Some(Tag::Function), true).color
        );
    }

    #[test]
    fn typst_formats_use_the_shared_editor_font_role() {
        for tag in [
            None,
            Some(Tag::Keyword),
            Some(Tag::String),
            Some(Tag::Error),
        ] {
            assert_eq!(format_for(tag, true).font_id, theme::editor_font());
            assert_eq!(format_for(tag, false).font_id, theme::editor_font());
        }
    }

    #[test]
    fn malformed_documents_are_highlighted_without_losing_text() {
        let mut highlighter = SyntaxHighlighter::default();
        let syntect = GenericSyntaxHighlighter::default();
        for source in [
            "#let unfinished =",
            "= *unterminated emphasis",
            "$ x + ",
            "#(missing: [bracket",
            "Unicode — λ and emoji 🙂\n#let x = ",
        ] {
            let job = highlighter.highlight(source, true, &syntect);
            assert_exact_mapping(&job, source);
        }
    }

    #[test]
    fn incremental_replacement_updates_tags_and_theme() {
        let mut highlighter = SyntaxHighlighter::default();
        let syntect = GenericSyntaxHighlighter::default();

        let markup = highlighter.highlight("as", true, &syntect);
        assert_eq!(format_at(&markup, 0).color, format_for(None, true).color);

        let source = "#let value = 42";
        highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::syntax_palette(true),
            None,
            &Default::default(),
        ));
        let dark = highlighter.highlight(source, true, &syntect);
        highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::syntax_palette(false),
            None,
            &Default::default(),
        ));
        let light = highlighter.highlight(source, false, &syntect);
        assert_exact_mapping(&dark, source);
        assert_exact_mapping(&light, source);
        assert_ne!(format_at(&dark, 1).color, format_at(&light, 1).color);

        // A repeat call exercises the completed LayoutJob cache.
        assert_eq!(highlighter.highlight(source, false, &syntect).text, source);
    }

    #[test]
    fn typst_overrides_flow_into_layout_formats() {
        let mut overrides = TypstStyleOverrides::default();
        let keyword = overrides.get_mut_or_default(TypstSyntaxRole::Keyword);
        keyword.foreground = Some(Rgba::rgb(1, 2, 3));
        keyword.background = Some(Rgba::from_rgba(4, 5, 6, 90));
        keyword.bold = Some(true);
        keyword.italic = Some(true);
        keyword.underline = Some(true);
        keyword.strikethrough = Some(true);

        let mut highlighter = SyntaxHighlighter::default();
        highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::syntax_palette(true),
            None,
            &overrides,
        ));
        let syntect = GenericSyntaxHighlighter::default();
        let source = "#let value = 1";
        let job = highlighter.highlight(source, true, &syntect);
        let format = format_at(&job, source.find("let").unwrap());

        assert_eq!(format.color, Color32::from_rgb(1, 2, 3));
        assert_eq!(
            format.background,
            Color32::from_rgba_unmultiplied(4, 5, 6, 90)
        );
        assert_eq!(format.font_id, theme::editor_font_with_weight(true));
        assert!(format.italics);
        assert_ne!(format.underline, Stroke::NONE);
        assert_ne!(format.strikethrough, Stroke::NONE);
        assert_exact_mapping(&job, source);
    }

    #[test]
    fn tagged_raw_blocks_delegate_the_payload_to_syntect() {
        let source = "```rust\nfn main() {}\n```";
        let mut syntect_theme = syntect::highlighting::Theme::default();
        syntect_theme.settings.foreground = Some(syntect::highlighting::Color {
            r: 3,
            g: 97,
            b: 211,
            a: 255,
        });
        let mut syntect = GenericSyntaxHighlighter::default();
        syntect.set_custom_theme(Some(syntect_theme));
        let mut highlighter = SyntaxHighlighter::default();
        let job = highlighter.highlight(source, true, &syntect);
        let keyword = format_at(&job, source.find("fn").unwrap());
        let raw_delimiter = format_at(&job, 0);

        assert_eq!(keyword.color, Color32::from_rgb(3, 97, 211));
        assert_ne!(keyword.color, raw_delimiter.color);
        assert_exact_mapping(&job, source);
    }

    #[test]
    fn unknown_raw_language_falls_back_to_typst_raw_style() {
        let source = "```not-a-language\nplain payload\n```";
        let job = highlight(source, true);
        let payload = format_at(&job, source.find("plain").unwrap());
        assert_eq!(payload.color, format_for(Some(Tag::Raw), true).color);
        assert_exact_mapping(&job, source);
    }

    #[test]
    fn empty_source_has_an_exact_empty_mapping() {
        let job = highlight("", true);
        assert_exact_mapping(&job, "");
    }

    #[test]
    fn hex_color_strings_become_readable_color_swatches() {
        let source = r##"#let accent = rgb("#4f8cff")"##;
        let job = highlight(source, true);
        let format = format_at(&job, source.find("#4f8cff").unwrap());

        assert_eq!(format.background, Color32::from_rgb(0x4f, 0x8c, 0xff));
        assert_eq!(format.color, Color32::BLACK);
        assert_exact_mapping(&job, source);
    }

    #[test]
    fn shorthand_and_alpha_hex_colors_are_parsed_safely() {
        assert_eq!(
            parse_hex_color_string(r##""#abc""##),
            Some(Color32::from_rgb(0xaa, 0xbb, 0xcc))
        );
        assert_eq!(
            parse_hex_color_string(r##""#10203080""##),
            Some(Color32::from_rgba_unmultiplied(0x10, 0x20, 0x30, 0x80))
        );
        assert_eq!(parse_hex_color_string(r##""#not-a-color""##), None);
        assert_eq!(contrast_text(Color32::BLACK), Color32::WHITE);
        assert_eq!(contrast_text(Color32::WHITE), Color32::BLACK);
    }
}
