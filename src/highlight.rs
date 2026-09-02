use eframe::egui::{Color32, FontFamily, FontId, Stroke, TextFormat, text::LayoutJob};
use typst_syntax::{LinkedNode, Source, Tag};

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
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self {
            parsed_source: Source::detached(String::new()),
            cached_dark_mode: true,
            cached_job: LayoutJob::default(),
            has_cache: false,
        }
    }
}

impl SyntaxHighlighter {
    pub fn highlight(&mut self, source: &str, dark_mode: bool) -> LayoutJob {
        let source_changed = self.parsed_source.text() != source;
        if self.has_cache && !source_changed && self.cached_dark_mode == dark_mode {
            return self.cached_job.clone();
        }

        if source_changed {
            // `replace` computes the common prefix/suffix and reparses only the
            // changed syntax-tree region. It is the incremental API supplied by
            // Typst itself.
            self.parsed_source.replace(source);
        }

        let mut job = LayoutJob::default();
        let root = LinkedNode::new(self.parsed_source.root());
        append_node(&mut job, &root, None, dark_mode);
        job.wrap.break_anywhere = false;

        // TextEdit requires the galley's byte positions to map exactly back to
        // the editable string. This catches accidental omission or injection if
        // Typst's syntax-tree representation changes in the future.
        debug_assert_eq!(job.text, source);

        self.cached_dark_mode = dark_mode;
        self.cached_job = job.clone();
        self.has_cache = true;
        job
    }
}

/// Walk leaves in source order. A tag on the closest node wins; this preserves
/// structural tags such as headings while allowing nested constructs (strong,
/// raw, interpolation, errors, and so on) to override them.
fn append_node(job: &mut LayoutJob, node: &LinkedNode<'_>, inherited: Option<Tag>, dark: bool) {
    let tag = typst_syntax::highlight(node).or(inherited);
    let text = node.leaf_text();

    if text.is_empty() {
        for child in node.children() {
            append_node(job, &child, tag, dark);
        }
    } else {
        let mut format = format_for(tag, dark);
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

fn format_for(tag: Option<Tag>, dark: bool) -> TextFormat {
    let plain = if dark {
        Color32::from_rgb(214, 219, 230)
    } else {
        Color32::from_rgb(52, 58, 70)
    };

    let color = if dark {
        match tag {
            None => plain,
            Some(Tag::Comment) => Color32::from_rgb(106, 122, 144),
            Some(Tag::Punctuation | Tag::MathGroupingParens | Tag::Operator) => {
                Color32::from_rgb(145, 215, 227)
            }
            Some(Tag::Escape | Tag::Number) => Color32::from_rgb(245, 169, 127),
            Some(Tag::Strong | Tag::Emph | Tag::MathDelimiter | Tag::MathOperator) => {
                Color32::from_rgb(244, 184, 228)
            }
            Some(Tag::Link | Tag::Function) => Color32::from_rgb(125, 196, 228),
            Some(Tag::Raw | Tag::String) => Color32::from_rgb(166, 218, 149),
            Some(Tag::Label | Tag::Ref) => Color32::from_rgb(139, 213, 202),
            Some(Tag::Heading | Tag::ListMarker | Tag::ListTerm) => {
                Color32::from_rgb(238, 212, 159)
            }
            Some(Tag::Keyword) => Color32::from_rgb(198, 160, 246),
            Some(Tag::Interpolated) => Color32::from_rgb(183, 189, 248),
            Some(Tag::Error) => Color32::from_rgb(237, 135, 150),
        }
    } else {
        match tag {
            None => plain,
            Some(Tag::Comment) => Color32::from_rgb(120, 126, 140),
            Some(Tag::Punctuation | Tag::MathGroupingParens | Tag::Operator) => {
                Color32::from_rgb(26, 112, 146)
            }
            Some(Tag::Escape | Tag::Number) => Color32::from_rgb(190, 88, 40),
            Some(Tag::Strong | Tag::Emph | Tag::MathDelimiter | Tag::MathOperator) => {
                Color32::from_rgb(158, 53, 137)
            }
            Some(Tag::Link | Tag::Function) => Color32::from_rgb(26, 112, 146),
            Some(Tag::Raw | Tag::String) => Color32::from_rgb(58, 128, 78),
            Some(Tag::Label | Tag::Ref) => Color32::from_rgb(20, 122, 111),
            Some(Tag::Heading | Tag::ListMarker | Tag::ListTerm) => Color32::from_rgb(145, 93, 16),
            Some(Tag::Keyword) => Color32::from_rgb(126, 69, 174),
            Some(Tag::Interpolated) => Color32::from_rgb(89, 77, 150),
            Some(Tag::Error) => Color32::from_rgb(190, 36, 54),
        }
    };

    let mut format = TextFormat {
        font_id: FontId::new(15.0, FontFamily::Monospace),
        color,
        ..Default::default()
    };

    match tag {
        Some(Tag::Emph) => format.italics = true,
        Some(Tag::Link) => format.underline = Stroke::new(1.0, color),
        Some(Tag::Error) => {
            format.background = if dark {
                Color32::from_rgba_unmultiplied(237, 135, 150, 34)
            } else {
                Color32::from_rgba_unmultiplied(190, 36, 54, 24)
            };
        }
        _ => {}
    }

    format
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
    let base = if dark { [30, 34, 43] } else { [250, 250, 252] };
    let alpha = color.a() as u16;
    let blend = |foreground: u8, background: u8| {
        ((foreground as u16 * alpha + background as u16 * (255 - alpha) + 127) / 255) as u8
    };
    Color32::from_rgb(
        blend(color.r(), base[0]),
        blend(color.g(), base[1]),
        blend(color.b(), base[2]),
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
        let mut highlighter = SyntaxHighlighter::default();
        let job = highlighter.highlight(source, true);
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
        let mut highlighter = SyntaxHighlighter::default();
        let job = highlighter.highlight(source, true);
        let as_byte = source.find(" as ").unwrap() + 1;

        assert_exact_mapping(&job, source);
        assert_eq!(
            format_at(&job, as_byte).color,
            format_for(Some(Tag::Keyword), true).color
        );
    }

    #[test]
    fn malformed_documents_are_highlighted_without_losing_text() {
        let mut highlighter = SyntaxHighlighter::default();
        for source in [
            "#let unfinished =",
            "= *unterminated emphasis",
            "$ x + ",
            "#(missing: [bracket",
            "Unicode — λ and emoji 🙂\n#let x = ",
        ] {
            let job = highlighter.highlight(source, true);
            assert_exact_mapping(&job, source);
        }
    }

    #[test]
    fn incremental_replacement_updates_tags_and_theme() {
        let mut highlighter = SyntaxHighlighter::default();

        let markup = highlighter.highlight("as", true);
        assert_eq!(format_at(&markup, 0).color, format_for(None, true).color);

        let source = "#let value = 42";
        let dark = highlighter.highlight(source, true);
        let light = highlighter.highlight(source, false);
        assert_exact_mapping(&dark, source);
        assert_exact_mapping(&light, source);
        assert_ne!(format_at(&dark, 1).color, format_at(&light, 1).color);

        // A repeat call exercises the completed LayoutJob cache.
        assert_eq!(highlighter.highlight(source, false).text, source);
    }

    #[test]
    fn empty_source_has_an_exact_empty_mapping() {
        let mut highlighter = SyntaxHighlighter::default();
        let job = highlighter.highlight("", true);
        assert_exact_mapping(&job, "");
    }

    #[test]
    fn hex_color_strings_become_readable_color_swatches() {
        let source = r##"#let accent = rgb("#4f8cff")"##;
        let mut highlighter = SyntaxHighlighter::default();
        let job = highlighter.highlight(source, true);
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
