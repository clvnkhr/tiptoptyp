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
        job.append(text, 0.0, format_for(tag, dark));
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
        Some(Tag::Error) => format.underline = Stroke::new(1.0, color),
        _ => {}
    }

    format
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
}
