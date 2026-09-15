//! Bracket families, nesting, and palettes, independent of editor/window state.
use std::ops::Range;

use eframe::egui::{
    Color32,
    text::{ByteIndex, LayoutJob, LayoutSection},
};
use serde::{Deserialize, Serialize};
use typst_syntax::{LinkedNode, SyntaxKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BracketFamily {
    Round,
    Square,
    Curly,
    Mixed,
}
impl BracketFamily {
    pub(crate) const ALL: [Self; 4] = [Self::Round, Self::Square, Self::Curly, Self::Mixed];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Round => "()",
            Self::Square => "[]",
            Self::Curly => "{}",
            Self::Mixed => "mixed",
        }
    }
    fn of(open: &str, close: &str) -> Option<Self> {
        Some(match (open, close) {
            ("(", ")") => Self::Round,
            ("[", "]") => Self::Square,
            ("{", "}") => Self::Curly,
            ("(" | "[" | "{", ")" | "]" | "}") => Self::Mixed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum BracketPalette {
    Spectrum,
    Forest,
    Sunset,
    Orchid,
}
impl BracketPalette {
    pub(crate) const ALL: [Self; 4] = [Self::Spectrum, Self::Forest, Self::Sunset, Self::Orchid];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Spectrum => "Spectrum",
            Self::Forest => "Forest",
            Self::Sunset => "Sunset",
            Self::Orchid => "Orchid",
        }
    }
    pub(crate) fn colors(self, dark: bool) -> [Color32; 4] {
        let hex = match (self, dark) {
            (Self::Spectrum, false) => [0x956000, 0x005fbb, 0xa62380, 0x00756a],
            (Self::Spectrum, true) => [0xf6c65b, 0x79bcff, 0xf29bd4, 0x67d6c3],
            (Self::Forest, false) => [0x197348, 0x007881, 0x526c00, 0x286298],
            (Self::Forest, true) => [0x86d99f, 0x6dd3dd, 0xc4d772, 0x8cbfe9],
            (Self::Sunset, false) => [0xb04b10, 0xac3158, 0x8c6120, 0x974f85],
            (Self::Sunset, true) => [0xffac73, 0xf58da7, 0xeacd80, 0xe9a0cf],
            (Self::Orchid, false) => [0x7950b6, 0x9e376b, 0x405bb2, 0x875186],
            (Self::Orchid, true) => [0xc6a5f5, 0xf3a0c8, 0xabb5ff, 0xd7a2d5],
        };
        hex.map(|hex| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RainbowBrackets {
    pub(crate) enabled: bool,
    pub(crate) palettes: [BracketPalette; 4],
}
impl Default for RainbowBrackets {
    fn default() -> Self {
        Self {
            enabled: true,
            palettes: BracketPalette::ALL,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BracketSpan {
    bytes: Range<usize>,
    family: BracketFamily,
    depth: usize,
}

fn spans(root: LinkedNode<'_>) -> Vec<BracketSpan> {
    fn walk(node: LinkedNode<'_>, mut depth: [usize; 4], out: &mut Vec<BracketSpan>) {
        let mut children = node.children();
        if let (Some(first), Some(last)) = (children.next(), children.next_back())
            && !first.get().diagnosis().errors
            && !last.get().diagnosis().errors
            && (node.kind() == SyntaxKind::MathDelimited
                || matches!(
                    (first.kind(), last.kind()),
                    (SyntaxKind::LeftParen, SyntaxKind::RightParen)
                        | (SyntaxKind::LeftBracket, SyntaxKind::RightBracket)
                        | (SyntaxKind::LeftBrace, SyntaxKind::RightBrace)
                ))
            && let Some(family) = BracketFamily::of(first.leaf_text(), last.leaf_text())
        {
            for bytes in [first.range(), last.range()] {
                out.push(BracketSpan {
                    bytes,
                    family,
                    depth: depth[family as usize],
                });
            }
            depth[family as usize] += 1;
        }
        for child in node.children() {
            walk(child, depth, out);
        }
    }
    let mut result = Vec::new();
    walk(root, [0; 4], &mut result);
    result.sort_unstable_by_key(|span| span.bytes.start);
    result
}

/// Called only when the highlighter's text/style cache misses. Preserve every
/// byte and every non-color property, including user syntax decorations.
pub(crate) fn apply(
    job: &mut LayoutJob,
    root: LinkedNode<'_>,
    settings: RainbowBrackets,
    dark: bool,
) {
    if !settings.enabled {
        return;
    }
    let brackets = spans(root);
    if brackets.is_empty() {
        return;
    }
    let mut current = 0;
    let mut sections = Vec::with_capacity(job.sections.len() + brackets.len() * 2);
    for section in std::mem::take(&mut job.sections) {
        let start = section.byte_range.start.0;
        let end = section.byte_range.end.0;
        let mut cursor = start;
        while current < brackets.len() && brackets[current].bytes.end <= start {
            current += 1;
        }
        for bracket in brackets[current..]
            .iter()
            .take_while(|bracket| bracket.bytes.start < end)
        {
            let left = bracket.bytes.start.max(cursor);
            let right = bracket.bytes.end.min(end);
            if left >= right {
                continue;
            }
            if cursor < left {
                let mut prefix = section.clone();
                prefix.byte_range = ByteIndex(cursor)..ByteIndex(left);
                prefix.leading_space = if cursor == start {
                    section.leading_space
                } else {
                    0.0
                };
                sections.push(prefix);
            }
            let mut format = section.format.clone();
            let colors = settings.palettes[bracket.family as usize].colors(dark);
            format.color = colors[bracket.depth % colors.len()];
            sections.push(LayoutSection {
                leading_space: if left == start {
                    section.leading_space
                } else {
                    0.0
                },
                byte_range: ByteIndex(left)..ByteIndex(right),
                format,
            });
            cursor = right;
        }
        if cursor < end {
            sections.push(LayoutSection {
                leading_space: if cursor == start {
                    section.leading_space
                } else {
                    0.0
                },
                byte_range: ByteIndex(cursor)..ByteIndex(end),
                format: section.format,
            });
        }
    }
    job.sections = sections;
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst_syntax::Source;

    #[test]
    fn family_labels_are_compact_for_settings_rows() {
        assert_eq!(BracketFamily::Round.label(), "()");
        assert_eq!(BracketFamily::Square.label(), "[]");
        assert_eq!(BracketFamily::Curly.label(), "{}");
        assert_eq!(BracketFamily::Mixed.label(), "mixed");
    }

    #[test]
    fn families_cycle_independently_and_siblings_keep_their_depth() {
        let source = Source::detached("#let x = (1, (2), { (3, [body]) })\n$ (a + (b]) ] $");
        let spans = spans(LinkedNode::new(source.root()));
        let opens: Vec<_> = spans
            .iter()
            .filter(|s| "([{".contains(&source.text()[s.bytes.clone()]))
            .map(|s| (s.family, s.depth))
            .collect();
        assert_eq!(
            opens,
            vec![
                (BracketFamily::Round, 0),
                (BracketFamily::Round, 1),
                (BracketFamily::Curly, 0),
                (BracketFamily::Round, 1),
                (BracketFamily::Square, 0),
                (BracketFamily::Round, 0),
                (BracketFamily::Mixed, 0)
            ]
        );
        let source = Source::detached("#text[outer #text[mid #text[inner]]]");
        let squares = super::spans(LinkedNode::new(source.root()));
        assert_eq!(
            squares
                .iter()
                .map(|span| (span.family, span.depth))
                .collect::<Vec<_>>(),
            vec![
                (BracketFamily::Square, 0),
                (BracketFamily::Square, 1),
                (BracketFamily::Square, 2),
                (BracketFamily::Square, 2),
                (BracketFamily::Square, 1),
                (BracketFamily::Square, 0),
            ]
        );
    }

    #[test]
    fn excludes_literals_comments_raw_escapes_and_unclosed_brackets() {
        for text in [
            "plain (prose)",
            "// [comment]",
            "#let x = \"(str)\"",
            "`{raw}`",
            "\\[escaped\\]",
            "#let x = (unclosed",
        ] {
            assert!(
                spans(LinkedNode::new(Source::detached(text).root())).is_empty(),
                "{text}"
            );
        }
    }
}
