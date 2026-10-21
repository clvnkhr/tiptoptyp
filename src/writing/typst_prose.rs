//! Keep Typst prose continuous through inline formatting and opaque equations.
use harper_core::{Punctuation, Span, Token, TokenKind, parsers::Parser};
use typst_syntax::{
    LinkedNode, SyntaxKind,
    ast::{Arg, Expr, Named, SetRule},
};

pub(super) struct TypstProse;

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct TextLanguage {
    pub lang: Option<String>,
    pub region: Option<String>,
}

pub(super) struct Parsed {
    pub tokens: Vec<Token>,
    pub language: TextLanguage,
}

struct Region {
    span: Span<char>,
    punctuation: Vec<Token>,
    opaque_end: usize,
}

impl TypstProse {
    /// Share our syntax tree between prose masking and language detection.
    /// Harper's underlying Typst parser still owns its separate parse.
    pub(super) fn parse_with_language(source: &[char]) -> Parsed {
        Self::parse_with_formatting(source, false)
    }

    pub(super) fn parse_skipping_formatting(source: &[char]) -> Vec<Token> {
        Self::parse_with_formatting(source, true).tokens
    }

    fn parse_with_formatting(source: &[char], skip_formatted: bool) -> Parsed {
        let text: String = source.iter().collect();
        let syntax = typst_syntax::Source::detached(text.as_str());
        let mut offsets = vec![0; text.len() + 1];
        for (scalar, (byte, ch)) in text.char_indices().enumerate() {
            offsets[byte..byte + ch.len_utf8()].fill(scalar);
            offsets[byte + ch.len_utf8()] = scalar + 1;
        }
        // Be conservative about explicitly redefined builtins. Name lookup for
        // arbitrary Typst programs is the compiler's job, not this prose parser.
        let mut shadowed: Vec<_> = syntax
            .root()
            .children()
            .filter_map(|node| node.cast::<typst_syntax::ast::LetBinding>())
            .flat_map(|binding| binding.kind().bindings())
            .filter(|name| name.as_str() == "std" || is_inline_formatter(name.as_str()))
            .map(|name| name.as_str().to_owned())
            .collect();
        shadowed.sort_unstable();
        shadowed.dedup();
        let mut regions = Vec::new();
        let mut formatting = Vec::new();
        let mut structural_edges = Vec::new();
        collect_regions(
            LinkedNode::new(syntax.root()),
            &offsets,
            &mut regions,
            &mut formatting,
            &mut structural_edges,
            &shadowed,
            skip_formatted,
        );
        merge_syntax_ranges(&mut formatting);
        structural_edges.sort_unstable();
        structural_edges.dedup();
        let language = text_language(syntax.root());
        // Harper isolates every ContentBlock, even an inline #emph[...] body.
        // Erase only recognized formatting syntax in a parser-only mirror. Its
        // scalar length is unchanged; original source and diagnostics are kept.
        let mirror = if formatting.is_empty() {
            std::borrow::Cow::Borrowed(source)
        } else {
            let mut mirror = source.to_vec();
            for range in &formatting {
                mirror[range.start..range.end].fill(' ');
            }
            std::borrow::Cow::Owned(mirror)
        };
        let tokens = replace_regions(harper_typst::Typst.parse(&mirror), regions);
        Parsed {
            tokens: omit_formatting_syntax(tokens, source, &formatting, &structural_edges),
            language,
        }
    }
}

impl Parser for TypstProse {
    fn parse(&self, source: &[char]) -> Vec<Token> {
        Self::parse_with_language(source).tokens
    }
}

fn text_language(root: &typst_syntax::SyntaxNode) -> TextLanguage {
    let mut language = TextLanguage::default();
    // Only unconditional document-level set rules determine the writing
    // language. Nested examples, strings and comments cannot change it.
    for node in root.children() {
        let Some(rule) = node.cast::<SetRule>() else {
            continue;
        };
        if !matches!(rule.target(), Expr::Ident(name) if name.as_str() == "text")
            || rule.condition().is_some()
        {
            continue;
        }
        for arg in rule.args().items() {
            if let Arg::Named(named) = arg
                && let Expr::Str(value) = named.expr()
            {
                match named.name().as_str() {
                    "lang" => language.lang = Some(value.get().as_str().to_ascii_lowercase()),
                    "region" => language.region = Some(value.get().as_str().to_ascii_lowercase()),
                    _ => {}
                }
            }
        }
    }
    language
}

fn collect_regions(
    node: LinkedNode<'_>,
    offsets: &[usize],
    regions: &mut Vec<Region>,
    formatting: &mut Vec<Span<char>>,
    structural_edges: &mut Vec<usize>,
    shadowed: &[String],
    skip_formatted: bool,
) -> bool {
    let equation = node.kind() == SyntaxKind::Equation;
    let technical_argument = node.get().cast::<Named>().is_some_and(|arg| {
        !matches!(
            arg.name().as_str(),
            "body" | "title" | "caption" | "alt" | "description"
        )
    });
    if equation || technical_argument {
        let range = node.range();
        let (punctuation, terminal) = if equation {
            equation_punctuation(&node, offsets)
        } else {
            (Vec::new(), None)
        };
        regions.push(Region {
            span: Span::new(offsets[range.start], offsets[range.end]),
            opaque_end: terminal.unwrap_or(offsets[range.end]),
            punctuation,
        });
        return false;
    }
    if skip_formatted
        && (matches!(node.kind(), SyntaxKind::Strong | SyntaxKind::Emph)
            || inline_body(&node, shadowed).is_some())
    {
        let range = node.range();
        let start = node
            .prev_sibling()
            .filter(|previous| {
                previous.kind() == SyntaxKind::Hash && previous.range().end == range.start
            })
            .map_or(range.start, |hash| hash.range().start);
        // This manual-check policy hides the whole recognized formatter. Do
        // not recurse and expose a nested formatter, math object or punctuation.
        regions.push(Region {
            span: Span::new(offsets[start], offsets[range.end]),
            opaque_end: offsets[range.end],
            punctuation: Vec::new(),
        });
        return false;
    }
    let mut structural = matches!(
        node.kind(),
        SyntaxKind::Heading | SyntaxKind::ListItem | SyntaxKind::EnumItem | SyntaxKind::TermItem
    ) || node
        .get()
        .cast::<typst_syntax::ast::Raw>()
        .is_some_and(|raw| raw.block());
    if structural {
        let range = node.range();
        structural_edges.extend([offsets[range.start], offsets[range.end]]);
    }
    for child in node.children() {
        structural |= collect_regions(
            child,
            offsets,
            regions,
            formatting,
            structural_edges,
            shadowed,
            skip_formatted,
        );
    }
    let body = inline_body(&node, shadowed);
    if !structural && let Some(body) = &body {
        let call = node.range();
        let start = node
            .prev_sibling()
            .filter(|previous| {
                previous.kind() == SyntaxKind::Hash && previous.range().end == call.start
            })
            .map_or(call.start, |hash| hash.range().start);
        let body = body.range();
        formatting.push(Span::new(offsets[start], offsets[body.start]));
        formatting.push(Span::new(offsets[body.end], offsets[call.end]));
    }
    // A formatter containing a block or an unknown call stays isolated. This
    // flag is computed in the existing traversal, not by rescanning each body.
    structural || (node.kind() == SyntaxKind::FuncCall && body.is_none())
}

/// Known inline content formatters can be erased for Harper's parser without
/// changing the rendered words. Leave arbitrary functions and block containers
/// isolated: their arguments need not be part of the surrounding sentence.
fn inline_body<'a>(node: &LinkedNode<'a>, shadowed: &[String]) -> Option<LinkedNode<'a>> {
    if node.parent()?.kind() != SyntaxKind::Markup || node.diagnosis().errors {
        return None;
    }
    let call = node.get().cast::<typst_syntax::ast::FuncCall>()?;
    let name = match call.callee() {
        Expr::Ident(name) if !shadowed.iter().any(|bound| bound == name.as_str()) => name,
        Expr::FieldAccess(access)
            if !shadowed.iter().any(|bound| bound == "std")
                && matches!(access.target(), Expr::Ident(namespace) if namespace.as_str() == "std") =>
        {
            access.field()
        }
        _ => return None,
    };
    if !is_inline_formatter(name.as_str()) {
        return None;
    }
    let args = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Args)?;
    let mut bodies = args.children().filter_map(|arg| {
        if arg.kind() == SyntaxKind::ContentBlock {
            Some(arg)
        } else if arg
            .get()
            .cast::<Named>()
            .is_some_and(|named| named.name().as_str() == "body")
        {
            arg.children()
                .find(|child| child.kind() == SyntaxKind::ContentBlock)
        } else {
            None
        }
    });
    let body = bodies.next()?;
    if bodies.next().is_some() {
        return None;
    }
    body.children()
        .find(|child| child.kind() == SyntaxKind::Markup)
}

fn is_inline_formatter(name: &str) -> bool {
    matches!(
        name,
        "emph"
            | "strong"
            | "text"
            | "underline"
            | "overline"
            | "strike"
            | "smallcaps"
            | "highlight"
            | "sub"
            | "super"
    )
}

fn merge_syntax_ranges(ranges: &mut Vec<Span<char>>) {
    ranges.sort_unstable_by_key(|range| range.start);
    let mut merged = 0;
    for index in 0..ranges.len() {
        let range = ranges[index];
        if range.is_empty() {
            continue;
        }
        if merged > 0 && range.start <= ranges[merged - 1].end {
            ranges[merged - 1].end = ranges[merged - 1].end.max(range.end);
        } else {
            ranges[merged] = range;
            merged += 1;
        }
    }
    ranges.truncate(merged);
}

/// Drop only parser-mirror syntax, retaining original scalar coordinates and
/// real whitespace. A single sweep avoids a token-by-wrapper Cartesian scan.
fn omit_formatting_syntax(
    tokens: Vec<Token>,
    source: &[char],
    ranges: &[Span<char>],
    structural_edges: &[usize],
) -> Vec<Token> {
    if ranges.is_empty() {
        return tokens;
    }
    fn retain_part(
        result: &mut Vec<Token>,
        original: &Token,
        source: &[char],
        structural_edges: &[usize],
        start: usize,
        end: usize,
    ) {
        if start >= end {
            return;
        }
        let mut token = original.clone();
        token.span = Span::new(start, end);
        let clipped_break = token.kind.is_paragraph_break() && token.span != original.span;
        if clipped_break || matches!(token.kind, TokenKind::Space(_) | TokenKind::Newline(_)) {
            let newlines = source[start..end].iter().filter(|&&ch| ch == '\n').count();
            let structural_boundary = clipped_break
                && ((end == original.span.end && structural_edges.binary_search(&end).is_ok())
                    || (start == original.span.start
                        && structural_edges.binary_search(&start).is_ok()));
            token.kind = if structural_boundary || (clipped_break && newlines >= 2) {
                TokenKind::ParagraphBreak
            } else if newlines > 0 {
                TokenKind::Newline(newlines)
            } else {
                TokenKind::Space(end - start)
            };
        }
        result.push(token);
    }
    let mut result = Vec::with_capacity(tokens.len());
    let mut first = 0;
    for token in tokens {
        // Remaining empty boundaries belong to unmodified content containers,
        // including ones whose first token is mirror whitespace. Keep them.
        if token.span.is_empty() {
            result.push(token);
            continue;
        }
        while first < ranges.len() && ranges[first].end <= token.span.start {
            first += 1;
        }
        let mut cursor = token.span.start;
        let mut index = first;
        while index < ranges.len() && ranges[index].start < token.span.end {
            let range = ranges[index];
            retain_part(
                &mut result,
                &token,
                source,
                structural_edges,
                cursor,
                range.start.min(token.span.end),
            );
            cursor = cursor.max(range.end);
            if cursor >= token.span.end {
                break;
            }
            index += 1;
        }
        retain_part(
            &mut result,
            &token,
            source,
            structural_edges,
            cursor.min(token.span.end),
            token.span.end,
        );
    }
    let mut written = 0;
    let mut gap_range = 0;
    for index in 0..result.len() {
        let token = result[index].clone();
        if written > 0 && matches!(token.kind, TokenKind::Newline(1)) {
            let previous = &result[written - 1];
            if matches!(previous.kind, TokenKind::Newline(1))
                && previous.span.end < token.span.start
            {
                while gap_range < ranges.len() && ranges[gap_range].end <= previous.span.end {
                    gap_range += 1;
                }
                if ranges
                    .get(gap_range)
                    .is_some_and(|range| range.start < token.span.start)
                {
                    // Two layout newlines separated by omitted inline syntax
                    // are not a blank paragraph. Retain one original anchor;
                    // Harper otherwise sums even nonadjacent newline spans.
                    continue;
                }
            }
        }
        result[written] = token;
        written += 1;
    }
    result.truncate(written);
    result
}

fn equation_punctuation(
    equation: &LinkedNode<'_>,
    offsets: &[usize],
) -> (Vec<Token>, Option<usize>) {
    // An unfinished equation has no reliable grammatical end yet.
    if equation
        .children()
        .next_back()
        .is_none_or(|child| child.kind() != SyntaxKind::Dollar)
    {
        return (Vec::new(), None);
    }
    let Some(body) = equation
        .children()
        .find(|child| child.kind() == SyntaxKind::Math)
    else {
        return (Vec::new(), None);
    };
    fn row_tail(row: &[LinkedNode<'_>], offsets: &[usize], punctuation: &mut Vec<Token>) {
        let start = punctuation.len();
        for (index, node) in row.iter().enumerate().rev() {
            if matches!(
                node.kind(),
                SyntaxKind::Space
                    | SyntaxKind::LineComment
                    | SyntaxKind::BlockComment
                    | SyntaxKind::Label
                    | SyntaxKind::Hash
            ) {
                continue;
            }
            // Arguments, tuples, strings, field access and decimal literals are
            // separate AST nodes. Only standalone punctuation at the row end
            // participates in prose. !/? remain mathematical notation.
            if node.kind() != SyntaxKind::MathText {
                break;
            }
            let kind = match node.get().leaf_text().as_str() {
                "." => Punctuation::Period,
                "," => Punctuation::Comma,
                ";" => Punctuation::Semicolon,
                ":" => Punctuation::Colon,
                _ => break,
            };
            let range = node.range();
            // The lexer groups triples as shorthand, but leaves dots in runs
            // of two/four as MathText. Such a run is notation, not a full stop.
            if kind == Punctuation::Period
                && row.get(index.wrapping_sub(1)).is_some_and(|previous| {
                    previous.range().end == range.start
                        && previous.get().leaf_text().as_str().ends_with('.')
                })
            {
                break;
            }
            punctuation.push(Token::new(
                Span::new(offsets[range.start], offsets[range.end]),
                TokenKind::Punctuation(kind),
            ));
        }
        punctuation[start..].reverse();
    }
    let mut row = Vec::new();
    let mut punctuation = Vec::new();
    for child in body.children() {
        if child.kind() == SyntaxKind::Linebreak {
            row_tail(&row, offsets, &mut punctuation);
            row.clear();
        } else {
            row.push(child);
        }
    }
    let row_start = punctuation.len();
    row_tail(&row, offsets, &mut punctuation);
    let terminal = (punctuation.len() > row_start).then(|| punctuation.last().unwrap().span.end);
    (punctuation, terminal)
}

fn replace_regions(mut tokens: Vec<Token>, regions: Vec<Region>) -> Vec<Token> {
    // Harper 2.11 emits ignored function arguments before admitted arguments
    // and set-rule conditions before arguments, even when they occur later in
    // the source. The region sweep needs source order to exclude every payload.
    // Leave the normal already-ordered stream untouched; stable sorting also
    // retains paragraph-boundary ordering for equal scalar starts.
    if tokens
        .windows(2)
        .any(|pair| pair[0].span.start > pair[1].span.start)
    {
        tokens.sort_by_key(|token| token.span.start);
    }
    let mut original = tokens.into_iter().peekable();
    let mut result = Vec::new();
    for region in regions {
        while original
            .peek()
            .is_some_and(|token| token.span.end <= region.span.start)
        {
            result.push(original.next().unwrap());
        }
        if let Some(token) = original
            .peek()
            .filter(|token| token.span.start < region.span.start)
        {
            let mut prefix = token.clone();
            prefix.span.end = region.span.start;
            result.push(prefix);
        }
        let mut cursor = region.span.start;
        for token in &region.punctuation {
            if cursor < token.span.start {
                result.push(Token::new(
                    Span::new(cursor, token.span.start),
                    TokenKind::Unlintable,
                ));
            }
            result.push(token.clone());
            cursor = token.span.end;
        }
        if cursor < region.opaque_end {
            result.push(Token::new(
                Span::new(cursor, region.opaque_end),
                TokenKind::Unlintable,
            ));
        }
        // After final punctuation, closing delimiters/labels are syntax only.
        // They must not be the first semantic token of the next sentence.
        while let Some(token) = original
            .peek_mut()
            .filter(|token| token.span.start < region.span.end)
        {
            if token.span.end > region.span.end {
                token.span.start = region.span.end;
                break;
            }
            original.next();
        }
    }
    result.extend(original);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use harper_core::TokenStringExt;

    fn parsed(source: &str) -> Vec<Token> {
        TypstProse.parse(&source.chars().collect::<Vec<_>>())
    }

    #[test]
    fn inline_formatting_keeps_one_sentence_and_original_word_spans() {
        for source in [
            "These #emph[estimates] are uniform.",
            "These #strong[estimates] are uniform.",
            "These #text(font: \"NonsenseFont\")[estimates] are uniform.",
            "These #emph[#strong[estimates]] are uniform.",
            "These #emph[] estimates are uniform.",
        ] {
            let chars: Vec<_> = source.chars().collect();
            let tokens = parsed(source);
            assert_eq!(
                tokens
                    .iter_sentences()
                    .filter(|sentence| sentence.iter().any(|token| token.kind.is_word()))
                    .count(),
                1,
                "formatting must not split prose: {source}: {tokens:?}"
            );
            assert!(
                !tokens.iter().any(|token| token.kind.is_unlintable()),
                "formatting syntax is not a prose object: {source}: {tokens:?}"
            );
            let words: Vec<_> = tokens
                .iter()
                .filter(|token| token.kind.is_word())
                .map(|token| token.get_str(&chars))
                .collect();
            assert_eq!(words, ["These", "estimates", "are", "uniform"], "{source}");
            assert!(
                tokens
                    .windows(2)
                    .all(|pair| pair[0].span.end <= pair[1].span.start),
                "{source}: {tokens:?}"
            );
        }
    }

    fn word_paragraphs(source: &str) -> Vec<Vec<String>> {
        let chars: Vec<_> = source.chars().collect();
        let tokens = parsed(source);
        assert!(
            tokens
                .iter()
                .all(|token| token.span.start <= token.span.end && token.span.end <= chars.len()),
            "{source}: {tokens:?}"
        );
        assert!(
            tokens
                .windows(2)
                .all(|pair| pair[0].span.end <= pair[1].span.start),
            "{source}: {tokens:?}"
        );
        tokens
            .iter_paragraphs()
            .map(|paragraph| {
                paragraph
                    .iter()
                    .filter(|token| token.kind.is_word())
                    .map(|token| token.get_str(&chars))
                    .collect::<Vec<_>>()
            })
            .filter(|words| !words.is_empty())
            .collect()
    }

    #[test]
    fn formatting_preserves_real_paragraphs_and_container_isolation() {
        assert_eq!(
            word_paragraphs("#emph[First paragraph.\n\nSecond paragraph.]"),
            [vec!["First", "paragraph"], vec!["Second", "paragraph"]]
        );
        for container in [
            "#figure([#emph[Visible body]], caption: [#strong[Separate caption]])",
            "#block[#emph[Visible body]]",
            "#custom[#strong[Visible body]]",
        ] {
            let source = format!("Before. {container} After.");
            let paragraphs = word_paragraphs(&source);
            assert_eq!(
                paragraphs.first().unwrap(),
                &["Before"],
                "{source}: {paragraphs:?}"
            );
            assert_eq!(
                paragraphs.last().unwrap(),
                &["After"],
                "{source}: {paragraphs:?}"
            );
            assert!(
                paragraphs.iter().any(|words| words == &["Visible", "body"]),
                "{source}: {paragraphs:?}"
            );
            if container.contains("caption") {
                assert!(
                    paragraphs
                        .iter()
                        .any(|words| words == &["Separate", "caption"]),
                    "{source}: {paragraphs:?}"
                );
            }
        }
        for source in [
            "Before.\n#emph[= A heading\nFollowing prose.]\nAfter.",
            "Before.\n#emph[- A list item\n- Another list item]\nAfter.",
        ] {
            assert!(word_paragraphs(source).len() >= 3, "{source}");
        }
    }

    #[test]
    fn formatting_keeps_math_code_and_technical_options_opaque() {
        let source = "é Before #text(font: \"NonsenseFont\", fill: rgb(\"badcolor\"), body: [these #emph[estimates] $NonsenseMath$ `NonsenseCode`]) after.";
        let chars: Vec<_> = source.chars().collect();
        let tokens = parsed(source);
        let words: Vec<_> = tokens
            .iter()
            .filter(|token| token.kind.is_word())
            .map(|token| token.get_str(&chars))
            .collect();
        assert_eq!(words, ["é", "Before", "these", "estimates", "after"]);
        for token in tokens
            .iter()
            .filter(|token| token.kind.is_whitespace() && !token.span.is_empty())
        {
            assert!(
                token.get_ch(&chars).iter().all(|ch| ch.is_whitespace()),
                "syntax cannot become whitespace: {token:?}"
            );
        }
        let equation = source.find("$NonsenseMath$").unwrap();
        let offset = source[..equation].chars().count();
        assert!(
            tokens
                .iter()
                .any(|token| token.kind.is_unlintable() && token.span.start == offset)
        );
        word_paragraphs(source);
    }

    #[test]
    fn formatting_handles_empty_multiline_and_deeply_nested_bodies() {
        for source in [
            "Before #emph[] after.",
            "Before #emph[#strong[]] after.",
            "Before #text(\n\nfont: \"Font\", body: [#emph[these words]]) after.",
            "Before #std.emph[these #std.strong[words]] after.",
        ] {
            assert_eq!(word_paragraphs(source).len(), 1, "{source}");
        }
        let source = format!(
            "Before {}these words{} after.",
            "#emph[".repeat(64),
            "]".repeat(64)
        );
        assert_eq!(
            word_paragraphs(&source),
            [vec!["Before", "these", "words", "after"]]
        );
        // Do not fabricate one contiguous word span across original syntax.
        let chars: Vec<_> = "A non#emph[autonomous] equation.".chars().collect();
        for token in TypstProse
            .parse(&chars)
            .iter()
            .filter(|token| token.kind.is_word())
        {
            assert!(token.get_ch(&chars).iter().all(|ch| ch.is_alphabetic()));
        }
    }

    #[test]
    fn multiline_formatting_does_not_invent_blank_paragraphs_in_harper_documents() {
        for source in [
            "These\n#emph[\n  estimates\n]\nare uniform.",
            "Before\n#emph[]\nafter.",
            "Before #emph[\n#strong[\nthese words\n]\n] after.",
        ] {
            assert_eq!(word_paragraphs(source).len(), 1, "{source}");
            let document = harper_core::Document::new_curated(source, &TypstProse);
            assert_eq!(
                document
                    .get_tokens()
                    .iter_paragraphs()
                    .filter(|paragraph| paragraph.iter().any(|token| token.kind.is_word()))
                    .count(),
                1,
                "Harper fixups must keep one paragraph: {source}: {:?}",
                document.get_tokens()
            );
        }
        assert_eq!(
            word_paragraphs("#emph[First\n\nSecond]"),
            [vec!["First"], vec!["Second"]]
        );
    }

    #[test]
    fn formatting_at_structural_edges_keeps_headings_and_lists_separate() {
        for source in [
            "Before\n#emph[]\n= Heading\nAfter.",
            "Before #emph[these words]\n= Heading\nAfter.",
            "Before\n#strong[]\n- List item\nAfter.",
            "= Heading\n#emph[]\nAfter.",
            "- List item\n#strong[]\nAfter.",
        ] {
            let paragraphs = word_paragraphs(source);
            for paragraph in &paragraphs {
                assert!(
                    !(paragraph.iter().any(|word| word == "Before")
                        && paragraph
                            .iter()
                            .any(|word| word == "Heading" || word == "List")),
                    "leading prose joined a structural block: {source}: {paragraphs:?}"
                );
                assert!(
                    !(paragraph.iter().any(|word| word == "After")
                        && paragraph
                            .iter()
                            .any(|word| word == "Heading" || word == "List")),
                    "trailing prose joined a structural block: {source}: {paragraphs:?}"
                );
            }
        }
    }

    #[test]
    fn explicitly_redefined_formatters_retain_container_boundaries() {
        let source = "#let emph(body) = block(body)\n\nBefore. #emph[Body] After.";
        let chars: Vec<_> = source.chars().collect();
        let tokens = parsed(source);
        // The upstream closure parser repeats its name token. Preserve its
        // existing admission rather than inventing a stronger ordering contract
        // for unrelated definitions while testing shadowed-call isolation.
        assert_eq!(tokens, harper_typst::Typst.parse(&chars));
        let paragraphs: Vec<Vec<_>> = tokens
            .iter_paragraphs()
            .map(|paragraph| {
                paragraph
                    .iter()
                    .filter(|token| token.kind.is_word())
                    .map(|token| token.get_str(&chars))
                    .collect()
            })
            .collect();
        assert!(
            paragraphs.iter().any(|words| words == &["Body"]),
            "{paragraphs:?}"
        );
        assert_eq!(
            word_paragraphs("#let emph = none\n\nBefore #std.emph[these words] after.")
                .last()
                .unwrap(),
            &["Before", "these", "words", "after"]
        );
    }

    #[test]
    fn incomplete_formatting_does_not_reinterpret_later_prose() {
        for source in [
            "Before #emph[words",
            "Before #emph[an unmatched $ equation] after.\n\nA final sentence.",
            "Before #strong[an unmatched _ marker] after.\n\nA final sentence.",
            "#let example = emph[Defined content]\n\nA final sentence.",
        ] {
            let chars: Vec<_> = source.chars().collect();
            let tokens = parsed(source);
            assert!(
                tokens.iter().all(
                    |token| token.span.start <= token.span.end && token.span.end <= chars.len()
                ),
                "{source}: {tokens:?}"
            );
            let syntax = typst_syntax::Source::detached(source);
            let mut offsets = vec![0; source.len() + 1];
            for (scalar, (byte, ch)) in source.char_indices().enumerate() {
                offsets[byte..byte + ch.len_utf8()].fill(scalar);
                offsets[byte + ch.len_utf8()] = scalar + 1;
            }
            let mut regions = Vec::new();
            let mut formatting = Vec::new();
            collect_regions(
                LinkedNode::new(syntax.root()),
                &offsets,
                &mut regions,
                &mut formatting,
                &mut Vec::new(),
                &[],
                false,
            );
            assert!(
                formatting.is_empty(),
                "unsupported/malformed syntax must retain original admission: {source}"
            );
            // An unfinished equation can already consume later prose. This fix
            // must preserve existing syntax recovery, not pretend it is valid.
            assert_eq!(
                tokens,
                replace_regions(harper_typst::Typst.parse(&chars), regions),
                "{source}"
            );
        }
    }

    #[test]
    fn language_metadata_and_prose_mask_share_the_document_syntax() {
        let source = concat!(
            "#set text(lang: \"EN\", region: \"US\")\n",
            "Before $ x = 1. $ After.\n",
            "#figure([Visible body], caption: [Visible caption])",
        );
        let chars: Vec<_> = source.chars().collect();
        let parsed = TypstProse::parse_with_language(&chars);
        assert_eq!(
            parsed.language,
            TextLanguage {
                lang: Some("en".into()),
                region: Some("us".into()),
            }
        );
        let words: Vec<_> = parsed
            .tokens
            .iter()
            .filter(|token| token.kind.is_word())
            .map(|token| token.get_str(&chars))
            .collect();
        assert_eq!(
            words,
            ["Before", "After", "Visible", "body", "Visible", "caption"]
        );
        let period = chars.iter().position(|&ch| ch == '.').unwrap();
        assert!(parsed.tokens.iter().any(|token| {
            token.kind.is_period() && token.span == Span::new(period, period + 1)
        }));
    }

    #[test]
    fn language_rules_ignore_examples_and_conditions_and_keep_last_literals() {
        let source = concat!(
            "#set text(lang: \"en\", region: \"us\")\n",
            "// #set text(lang: \"fr\")\n",
            "#let example() = { set text(lang: \"de\") }\n",
            "#set text(lang: \"uk\") if true\n",
            "#set text(region: \"GB\")\n",
            "#set text(region: dynamically_chosen)\n",
        );
        let parsed = TypstProse::parse_with_language(&source.chars().collect::<Vec<_>>());
        assert_eq!(
            parsed.language,
            TextLanguage {
                lang: Some("en".into()),
                region: Some("gb".into()),
            }
        );
    }

    #[test]
    fn display_math_sentence_punctuation_is_preserved_at_its_original_location() {
        let source = "The estimate gives\n$ x = 1. $\nThe conclusion follows.";
        let characters: Vec<_> = source.chars().collect();
        let tokens = parsed(source);
        let sentences: Vec<_> = tokens
            .iter_sentences()
            .filter(|tokens| tokens.iter().any(|token| token.kind.is_word()))
            .collect();
        assert_eq!(sentences.len(), 2, "{source}: {tokens:?}");
        let location = characters.iter().position(|&ch| ch == '.').unwrap();
        assert!(
            tokens
                .iter()
                .any(|token| token.span == Span::new(location, location + 1)
                    && token.kind.is_period())
        );
        assert!(
            sentences[1]
                .iter()
                .find(|token| !token.kind.is_whitespace())
                .unwrap()
                .kind
                .is_word()
        );
    }

    #[test]
    fn mathematical_notation_does_not_invent_sentence_boundaries() {
        for equation in [
            "x = 1",
            "x = 1,",
            "dot.op(x, y) = 1.25",
            "(x,y)",
            "n!",
            "x?",
            "x..",
            "x...",
            "x....",
            "text(\"words.\")",
            "x // comment.\n",
        ] {
            let source = format!("The estimate gives\n$ {equation} $\n  which proves the result.");
            let tokens = parsed(&source);
            assert_eq!(
                tokens
                    .iter_sentences()
                    .filter(|tokens| tokens.iter().any(|token| token.kind.is_word()))
                    .count(),
                1,
                "{source}: {tokens:?}"
            );
            assert_eq!(
                tokens.iter().filter(|token| token.kind.is_period()).count(),
                1,
                "{source}: {tokens:?}"
            );
        }
    }

    #[test]
    fn commas_labels_unicode_and_aligned_rows_keep_original_spans() {
        let source = "é Before $ x = y, \\ z = x. #<eq:test> $ After.";
        let chars: Vec<_> = source.chars().collect();
        let tokens = parsed(source);
        for (index, ch) in chars
            .iter()
            .enumerate()
            .filter(|(_, ch)| matches!(ch, ',' | '.'))
        {
            assert!(
                tokens
                    .iter()
                    .any(|token| token.span == Span::new(index, index + 1)
                        && token.kind.is_punctuation()),
                "{ch} at {index}: {tokens:?}"
            );
        }
        assert!(
            tokens
                .windows(2)
                .all(|pair| pair[0].span.end <= pair[1].span.start)
        );
        assert!(tokens.iter().all(|token| token.span.end <= chars.len()));
    }

    #[test]
    fn true_paragraph_heading_and_list_boundaries_are_preserved() {
        for separator in ["\n\n", "\n= Another result\n", "\n- Another result\n"] {
            let source = format!("The estimate gives\n$ x = 1 ${separator}The conclusion follows.");
            let tokens = parsed(&source);
            assert!(
                tokens
                    .iter()
                    .any(|token| token.kind == TokenKind::ParagraphBreak),
                "{source}: {tokens:?}"
            );
        }
    }

    #[test]
    fn technical_arguments_are_not_english_but_text_arguments_are() {
        let source = "#set text(lang: \"zzzz\", font: \"nonnword\")\n#figure([Text body], caption: [Visible caption])";
        let chars: Vec<_> = source.chars().collect();
        let words: Vec<_> = TypstProse
            .parse(&chars)
            .iter()
            .filter(|token| token.kind.is_word())
            .map(|token| token.get_str(&chars))
            .collect();
        assert!(!words.contains(&"zzzz".into()) && !words.contains(&"nonnword".into()));
        assert!(words.iter().any(|word| word == "Visible"));
    }

    #[test]
    fn unfinished_math_keeps_its_notation_opaque() {
        let source = "We obtain $ x = 1.";
        let tokens = parsed(source);
        assert!(!tokens.iter().any(|token| token.kind.is_period()));
        assert!(
            tokens
                .windows(2)
                .all(|pair| pair[0].span.end <= pair[1].span.start)
        );
    }
    #[test]
    fn upstream_argument_token_order_does_not_leak_technical_words_or_hide_captions() {
        for source in [
            "#set text(lang: \"zzzz\", font: \"nonnword\") if true",
            "Before. #image(alt: [Caption $ x = 1. $ The conclusion.], \"file.png\") After.",
        ] {
            let chars: Vec<_> = source.chars().collect();
            let tokens = parsed(source);
            let words: Vec<_> = tokens
                .iter()
                .filter(|token| token.kind.is_word())
                .map(|token| token.get_str(&chars))
                .collect();
            assert!(
                !words
                    .iter()
                    .any(|word| matches!(word.as_str(), "zzzz" | "nonnword" | "file" | "png")),
                "technical payloads must stay excluded: {source}: {tokens:?}"
            );
            assert!(
                tokens
                    .windows(2)
                    .all(|pair| pair[0].span.end <= pair[1].span.start),
                "scalar spans must remain monotonic: {source}: {tokens:?}"
            );
            if source.contains("Caption") {
                assert!(words.iter().any(|word| word == "Caption"));
                assert!(words.iter().any(|word| word == "conclusion"));
                assert_eq!(
                    tokens.iter().filter(|token| token.kind.is_period()).count(),
                    4,
                    "equation punctuation must survive exactly once: {source}: {tokens:?}"
                );
            }
        }
    }
}
