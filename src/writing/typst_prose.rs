//! Keep Typst equations opaque while retaining their grammatical punctuation.
use harper_core::{Punctuation, Span, Token, TokenKind, parsers::Parser};
use typst_syntax::{LinkedNode, SyntaxKind, ast::Named};

pub(super) struct TypstProse;

struct Region {
    span: Span<char>,
    punctuation: Vec<Token>,
    opaque_end: usize,
}

impl Parser for TypstProse {
    fn parse(&self, source: &[char]) -> Vec<Token> {
        let text: String = source.iter().collect();
        let syntax = typst_syntax::Source::detached(text.as_str());
        let mut offsets = vec![0; text.len() + 1];
        for (scalar, (byte, ch)) in text.char_indices().enumerate() {
            offsets[byte..byte + ch.len_utf8()].fill(scalar);
            offsets[byte + ch.len_utf8()] = scalar + 1;
        }
        let mut regions = Vec::new();
        collect_regions(LinkedNode::new(syntax.root()), &offsets, &mut regions);
        let tokens = harper_typst::Typst.parse(source);
        replace_regions(tokens, regions)
    }
}

fn collect_regions(node: LinkedNode<'_>, offsets: &[usize], regions: &mut Vec<Region>) {
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
    } else {
        for child in node.children() {
            collect_regions(child, offsets, regions);
        }
    }
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
