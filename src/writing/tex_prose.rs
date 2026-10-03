//! Mask equation syntax while retaining prose commands and scalar coordinates.
//! Uses the same tolerant math scanner as indentation guides; no macro expansion.
use crate::document::DocumentKind;

pub(super) struct TexProse;

impl harper_core::parsers::Parser for TexProse {
    fn parse(&self, source: &[char]) -> Vec<harper_core::Token> {
        use harper_core::{Masker, Span, Token, TokenKind};
        let mask = MathMask.create_mask(source);
        let mut tokens = Vec::new();
        let mut cursor = 0;
        for (span, content) in mask.iter_allowed(source) {
            if cursor < span.start {
                // An equation occupies the gap. Keep a non-prose token there so
                // removing it cannot invent adjacent words or whitespace before
                // punctuation (e.g. `with $x$.`).
                tokens.push(Token::new(
                    Span::new(cursor, span.start),
                    TokenKind::Unlintable,
                ));
            }
            let mut prose = harper_tex::TeX::default().parse(content);
            for token in &mut prose {
                token.span.push_by(span.start);
            }
            tokens.extend(prose);
            cursor = span.end;
        }
        if cursor < source.len() {
            tokens.push(Token::new(
                Span::new(cursor, source.len()),
                TokenKind::Unlintable,
            ));
        }
        tokens
    }
}

pub(super) struct MathMask;

impl harper_core::Masker for MathMask {
    fn create_mask(&self, original: &[char]) -> harper_core::Mask {
        let source: String = original.iter().collect();
        let mut allowed = vec![true; original.len()];
        let offsets: Vec<usize> = source.char_indices().map(|(byte, _)| byte).collect();
        let mut covered = 0;
        for block in crate::editor_math_guides::blocks(&source, DocumentKind::Tex) {
            // Outer blocks precede nested blocks. Process each source character once.
            if block.range.start < covered {
                continue;
            }
            covered = block.range.end;
            let start = offsets.partition_point(|&byte| byte < block.range.start);
            let end = offsets.partition_point(|&byte| byte < block.range.end);
            allowed[start..end].fill(false);
            reveal_text(&original[start..end], &mut allowed[start..end]);
        }
        let mut mask = harper_core::Mask::new_blank();
        let mut cursor = 0;
        while cursor < allowed.len() {
            if !allowed[cursor] {
                cursor += 1;
                continue;
            }
            let start = cursor;
            while cursor < allowed.len() && allowed[cursor] {
                cursor += 1;
            }
            mask.push_allowed(harper_core::Span::new(start, cursor));
        }
        mask
    }
}

fn reveal_text(original: &[char], allowed: &mut [bool]) {
    let mut cursor = 0;
    while cursor < original.len() {
        if original[cursor] == '%' {
            while cursor < original.len() && original[cursor] != '\n' {
                cursor += 1;
            }
        } else if original[cursor] == '\\' {
            cursor += 1;
            let start = cursor;
            while cursor < original.len() && original[cursor].is_ascii_alphabetic() {
                cursor += 1;
            }
            if cursor == start {
                cursor = (cursor + 1).min(original.len());
                continue;
            }
            let name: String = original[start..cursor].iter().collect();
            if !matches!(
                name.as_str(),
                "text"
                    | "textrm"
                    | "textnormal"
                    | "textbf"
                    | "textit"
                    | "textsf"
                    | "texttt"
                    | "mbox"
                    | "hbox"
                    | "intertext"
                    | "shortintertext"
            ) {
                continue;
            }
            while cursor < original.len() && original[cursor].is_whitespace() {
                cursor += 1;
            }
            if original.get(cursor) != Some(&'{') {
                continue;
            }
            cursor += 1;
            let body = cursor;
            let mut depth = 1;
            while cursor < original.len() {
                match original[cursor] {
                    '\\' => cursor = (cursor + 2).min(original.len()),
                    '%' => {
                        while cursor < original.len() && original[cursor] != '\n' {
                            cursor += 1;
                        }
                    }
                    '{' => {
                        depth += 1;
                        cursor += 1;
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        cursor += 1;
                    }
                    _ => cursor += 1,
                }
            }
            // Preserve nested prose commands for Harper's normal TeX parser.
            allowed[body..cursor].fill(true);
            cursor = (cursor + 1).min(original.len());
        } else {
            cursor += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harper_core::Masker;

    #[test]
    fn mask_preserves_unicode_offsets_and_nested_groups() {
        let source = "é before \\[ xyz \\text{café \\emph{words} \\{yes\\}} + xyz \\] after";
        let original: Vec<_> = source.chars().collect();
        let mask = MathMask.create_mask(&original);
        let text: String = mask
            .iter_allowed(&original)
            .flat_map(|(_, text)| text.iter())
            .collect();
        assert!(text.contains("café \\emph{words} \\{yes\\}"));
        assert!(!text.contains("xyz"));
        let body_start = source[..source.find("café").unwrap()].chars().count();
        assert!(
            mask.iter_allowed(&original)
                .any(|(span, _)| span.start == body_start)
        );
    }

    #[test]
    fn escaped_commands_comments_and_nested_math_do_not_restore_equation_text() {
        let source = "\\[\\\\text{nonwordxyz} % \\text{nonwordxyz}\n\\begin{aligned} x & y \\text{real prose} \\end{aligned}\\]";
        let original: Vec<_> = source.chars().collect();
        let mask = MathMask.create_mask(&original);
        let text: String = mask
            .iter_allowed(&original)
            .flat_map(|(_, text)| text.iter())
            .collect();
        assert!(!text.contains("nonwordxyz"));
        assert_eq!(text, "real prose");
    }
}
