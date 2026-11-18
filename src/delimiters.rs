//! Language-aware delimiter matching, without interpreting comments or literals.
use std::ops::Range;

use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

mod tex;
pub(crate) use tex::{TexDelimiterIndex, tex_ignored_ranges};

/// Typst's AST identifies valid pairs once per edited revision. Flat markup
/// can contain many siblings, so warm caret queries use interval/end-point
/// indices rather than repeatedly asking the syntax tree to find a leaf.
#[derive(Debug, Default)]
pub(crate) struct TypstDelimiterIndex {
    endpoints: Vec<[Range<usize>; 2]>,
    enclosing: EnclosingPairs,
}
impl TypstDelimiterIndex {
    pub(crate) fn new(source: &Source) -> Self {
        let mut pending = vec![LinkedNode::new(source.root())];
        let mut pairs = Vec::new();
        while let Some(node) = pending.pop() {
            if let Some(pair) = pair_for_node(&node) {
                pairs.push(pair);
            }
            pending.extend(node.children());
        }
        let mut endpoints = Vec::with_capacity(pairs.len() * 2);
        for pair in &pairs {
            endpoints.push(pair.clone());
            endpoints.push([pair[1].clone(), pair[0].clone()]);
        }
        endpoints.sort_unstable_by_key(|pair| pair[0].start);
        Self {
            endpoints,
            enclosing: EnclosingPairs::new(pairs),
        }
    }

    fn endpoint_at(&self, byte: usize) -> Option<[Range<usize>; 2]> {
        let at = self.endpoints.partition_point(|pair| pair[0].start <= byte);
        let pair = self.endpoints.get(at.checked_sub(1)?)?;
        if !pair[0].contains(&byte) {
            return None;
        }
        Some(if pair[0].start < pair[1].start {
            pair.clone()
        } else {
            [pair[1].clone(), pair[0].clone()]
        })
    }

    pub(crate) fn pair_at_caret(&self, byte: usize) -> Option<[Range<usize>; 2]> {
        self.endpoint_at(byte)
            .or_else(|| {
                byte.checked_sub(1)
                    .and_then(|previous| self.endpoint_at(previous))
            })
            .or_else(|| self.enclosing.containing(byte))
    }
}

/// Complete pairs indexed once per source revision. The implicit balanced tree
/// stores subtree closing bounds, so a caret lookup skips completed siblings
/// rather than walking backward through the source or its delimiter list.
#[derive(Debug, Default)]
struct EnclosingPairs {
    pairs: Vec<[Range<usize>; 2]>,
    maximum_end: Vec<usize>,
}
impl EnclosingPairs {
    fn new(mut pairs: Vec<[Range<usize>; 2]>) -> Self {
        pairs.sort_unstable_by_key(|pair| pair[0].start);
        let mut index = Self {
            maximum_end: vec![0; pairs.len()],
            pairs,
        };
        fn build(index: &mut EnclosingPairs, range: Range<usize>) -> usize {
            if range.is_empty() {
                return 0;
            }
            let middle = range.start + range.len() / 2;
            let end = index.pairs[middle][1]
                .end
                .max(build(index, range.start..middle))
                .max(build(index, middle + 1..range.end));
            index.maximum_end[middle] = end;
            end
        }
        let length = index.pairs.len();
        build(&mut index, 0..length);
        index
    }

    fn containing(&self, byte: usize) -> Option<[Range<usize>; 2]> {
        self.query(byte).0.map(|at| self.pairs[at].clone())
    }

    fn query(&self, byte: usize) -> (Option<usize>, usize) {
        fn find(
            index: &EnclosingPairs,
            range: Range<usize>,
            byte: usize,
            visits: &mut usize,
        ) -> Option<usize> {
            if range.is_empty() {
                return None;
            }
            *visits += 1;
            let middle = range.start + range.len() / 2;
            if index.maximum_end[middle] <= byte || index.pairs[range.start][0].start > byte {
                return None;
            }
            if let Some(at) = find(index, middle + 1..range.end, byte, visits) {
                return Some(at);
            }
            let pair = &index.pairs[middle];
            if pair[0].start <= byte && byte < pair[1].end {
                return Some(middle);
            }
            find(index, range.start..middle, byte, visits)
        }
        let mut visits = 0;
        (find(self, 0..self.pairs.len(), byte, &mut visits), visits)
    }
}

pub(crate) fn pair_at(source: &Source, byte: usize) -> Option<[Range<usize>; 2]> {
    let leaf = LinkedNode::new(source.root()).leaf_at(byte, Side::After)?;
    if !leaf.range().contains(&byte) || leaf.get().diagnosis().errors {
        return None;
    }
    let pair = if matches!(leaf.kind(), SyntaxKind::Str | SyntaxKind::Label) {
        pair_for_node(&leaf)
    } else {
        pair_for_node(leaf.parent()?)
    }?;
    pair.iter()
        .any(|endpoint| endpoint.contains(&byte))
        .then_some(pair)
}

fn pair_for_node(node: &LinkedNode<'_>) -> Option<[Range<usize>; 2]> {
    let range = node.range();
    // These tokens contain their delimiters rather than separate leaves.
    if matches!(node.kind(), SyntaxKind::Str | SyntaxKind::Label) {
        if node.get().diagnosis().errors {
            return None;
        }
        let text = node.leaf_text();
        let expected = if node.kind() == SyntaxKind::Str {
            ('"', '"')
        } else {
            ('<', '>')
        };
        if text.len() >= 2 && text.starts_with(expected.0) && text.ends_with(expected.1) {
            return Some([range.start..range.start + 1, range.end - 1..range.end]);
        }
        return None;
    }
    let mut children = node.children();
    let first = children.next()?;
    let last = children.next_back()?;
    if first.get().diagnosis().errors || last.get().diagnosis().errors {
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
        || node.kind() == SyntaxKind::MathDelimited;
    paired.then(|| [first.range(), last.range()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair_at_caret(source: &Source, byte: usize) -> Option<[Range<usize>; 2]> {
        TypstDelimiterIndex::new(source).pair_at_caret(byte)
    }

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

    #[test]
    fn caret_matching_uses_nearest_complete_syntax_and_outer_literal_markers() {
        for (text, body, left, right) in [
            (
                "#text[outer #text[inner body] tail]",
                "body",
                "[inner",
                "] tail",
            ),
            ("#text[outer #text[inner body] tail]", "tail", "[outer", "]"),
            ("#let x = (1, /* [fake] */ 2)", "fake", "(", ")"),
            ("#let x = (\"[fake]\", 2)", "fake", "\"", "\""),
            ("#text[`[fake]`]", "fake", "`", "`"),
            ("#text[*strong body*]", "body", "*", "*"),
            ("$ (alpha + beta] $", "alpha", "(", "]"),
        ] {
            let source = Source::detached(text);
            let start = text.find(left).unwrap();
            let end = text.rfind(right).unwrap();
            assert_eq!(
                pair_at_caret(&source, text.find(body).unwrap() + 1),
                Some([start..start + 1, end..end + 1]),
                "{text}"
            );
        }
        let text = "#text[outer #text[inner] tail]";
        let source = Source::detached(text);
        let outer = text.find('[').unwrap();
        let inner = text.rfind('[').unwrap();
        let inner_end = text.find(']').unwrap();
        let outer_end = text.rfind(']').unwrap();
        assert_eq!(
            pair_at_caret(&source, inner),
            Some([inner..inner + 1, inner_end..inner_end + 1])
        );
        assert_eq!(
            pair_at_caret(&source, inner_end + 1),
            Some([inner..inner + 1, inner_end..inner_end + 1])
        );
        assert_eq!(
            pair_at_caret(&source, inner_end + 2),
            Some([outer..outer + 1, outer_end..outer_end + 1])
        );
        assert_eq!(
            pair_at_caret(&source, text.len()),
            Some([outer..outer + 1, outer_end..outer_end + 1])
        );
        assert_eq!(pair_at_caret(&source, usize::MAX), None);
        for malformed in ["#text[unclosed body", "#let x = \"unclosed body"] {
            assert_eq!(
                pair_at_caret(
                    &Source::detached(malformed),
                    malformed.find("body").unwrap()
                ),
                None,
                "{malformed}"
            );
        }
        let text = "$ (unclosed body $";
        assert_eq!(
            pair_at_caret(&Source::detached(text), text.find("body").unwrap()),
            Some([0..1, text.len() - 1..text.len()]),
            "an invalid inner opener does not hide the complete surrounding math pair"
        );
    }

    #[test]
    fn caret_queries_prune_large_syntax_and_pair_indices() {
        let text = format!("#text[{}target]", "#text[sibling] ".repeat(20_000));
        let source = Source::detached(text.clone());
        let index = TypstDelimiterIndex::new(&source);
        let byte = text.find("target").unwrap() + 2;
        assert_eq!(
            index.pair_at_caret(byte),
            Some([5..6, text.len() - 1..text.len()])
        );
        let (_, visits) = index.enclosing.query(byte);
        assert!(visits <= 64, "indexed syntax query: {visits}");
        let count = 100_000;
        let mut pairs = vec![[0..1, count * 4 + 2..count * 4 + 3]];
        pairs.extend((0..count).map(|at| [at * 4 + 1..at * 4 + 2, at * 4 + 3..at * 4 + 4]));
        let index = EnclosingPairs::new(pairs);
        for caret in [2, count * 2, count * 4 + 1] {
            let (found, visits) = index.query(caret);
            assert!(found.is_some());
            assert!(
                visits <= 64,
                "bounded interval-tree query at {caret}: {visits}"
            );
        }
        assert_eq!(index.containing(usize::MAX), None);
        let nested = EnclosingPairs::new(
            (0..count)
                .map(|at| [at..at + 1, count * 2 - at..count * 2 - at + 1])
                .collect(),
        );
        let (found, visits) = nested.query(count);
        assert_eq!(found, Some(count - 1));
        assert!(
            visits <= 64,
            "deep source nesting still uses a balanced query: {visits}"
        );
    }
}
