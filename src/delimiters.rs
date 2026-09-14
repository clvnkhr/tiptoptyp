//! Delimiter matching from Typst syntax, without interpreting comments or text.
use std::ops::Range;

use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

pub(crate) fn pair_at(source: &Source, byte: usize) -> Option<[Range<usize>; 2]> {
    let leaf = LinkedNode::new(source.root()).leaf_at(byte, Side::After)?;
    if !leaf.range().contains(&byte) || leaf.get().diagnosis().errors {
        return None;
    }
    let range = leaf.range();
    // These tokens contain their delimiters rather than separate leaves.
    if matches!(leaf.kind(), SyntaxKind::Str | SyntaxKind::Label) {
        let text = leaf.leaf_text();
        let expected = if leaf.kind() == SyntaxKind::Str {
            ('"', '"')
        } else {
            ('<', '>')
        };
        if text.len() >= 2 && text.starts_with(expected.0) && text.ends_with(expected.1) {
            let pair = [range.start..range.start + 1, range.end - 1..range.end];
            return pair
                .iter()
                .any(|endpoint| endpoint.contains(&byte))
                .then_some(pair);
        }
        return None;
    }
    let parent = leaf.parent()?;
    let mut children = parent.children();
    let first = children.next()?;
    let last = children.next_back()?;
    if first.offset() != leaf.offset() && last.offset() != leaf.offset() {
        return None;
    }
    let paired = matches!(
        (first.kind(), last.kind()),
        (SyntaxKind::LeftParen, SyntaxKind::RightParen)
            | (SyntaxKind::LeftBracket, SyntaxKind::RightBracket)
            | (SyntaxKind::LeftBrace, SyntaxKind::RightBrace)
            | (SyntaxKind::Dollar, SyntaxKind::Dollar)
            | (SyntaxKind::Star, SyntaxKind::Star)
            | (SyntaxKind::Underscore, SyntaxKind::Underscore)
    ) || (first.kind() == SyntaxKind::RawDelim
        && last.kind() == SyntaxKind::RawDelim
        && first.leaf_text() == last.leaf_text())
        || parent.kind() == SyntaxKind::MathDelimited;
    paired.then(|| [first.range(), last.range()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_syntax_pairs_including_math_and_raw_delimiters() {
        for (text, left, right) in [
            ("#let x = (1, (2, 3))", "(", ")"),
            ("#let x = { [body] }", "{", "}"),
            ("#text[content]", "[", "]"),
            ("$ alpha + beta $", "$", "$"),
            ("$ (a + b] $", "(", "]"),
            ("#let x = \"a [bracket]\"", "\"", "\""),
            ("= Heading <label>", "<", ">"),
            ("*strong*", "*", "*"),
            ("_emphasis_", "_", "_"),
            ("```typ\n#let x = []\n```", "```", "```"),
        ] {
            let parsed = Source::detached(text);
            let start = text.find(left).unwrap();
            let end = text.rfind(right).unwrap();
            let expected = [start..start + left.len(), end..end + right.len()];
            assert_eq!(
                pair_at(&parsed, start),
                Some(expected.clone()),
                "{text}: {:#?}",
                parsed.root()
            );
            assert_eq!(pair_at(&parsed, end), Some(expected), "{text}");
        }
    }

    #[test]
    fn ignores_comments_escapes_raw_payloads_strings_and_unclosed_pairs() {
        for (text, marker) in [
            ("// (comment)\n#let x = (1)", "("),
            ("/* [comment] */", "["),
            ("#let x = \"[string]\"", "["),
            ("`[raw]`", "["),
            ("\\[escaped\\]", "["),
            ("#let x = (1, 2", "("),
            ("$ unclosed", "$"),
            ("#let x = \"unclosed", "\""),
            ("#let x = \"escaped\\\"", "\""),
            ("$ (unclosed $", "("),
            ("normal prose (text)", "("),
        ] {
            assert_eq!(
                pair_at(&Source::detached(text), text.find(marker).unwrap()),
                None,
                "{text}"
            );
        }
    }
}
