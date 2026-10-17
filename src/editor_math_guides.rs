//! Source alignment is math syntax, not indentation-based code nesting.
use crate::document::DocumentKind;
use std::ops::Range;

#[derive(Debug)]
pub(crate) struct MathBlock {
    pub(crate) range: Range<usize>,
    pub(crate) markers: Vec<usize>,
}

pub(crate) fn blocks(source: &str, kind: DocumentKind) -> Vec<MathBlock> {
    match kind {
        DocumentKind::Typst => {
            fn visit(node: typst_syntax::LinkedNode<'_>, result: &mut Vec<MathBlock>) {
                use typst_syntax::SyntaxKind;
                if node.kind() == SyntaxKind::Equation {
                    fn markers(node: typst_syntax::LinkedNode<'_>, result: &mut Vec<usize>) {
                        if node.kind() == SyntaxKind::MathAlignPoint {
                            result.push(node.range().start);
                        }
                        for child in node.children() {
                            markers(child, result);
                        }
                    }
                    let mut points = Vec::new();
                    markers(node.clone(), &mut points);
                    result.push(MathBlock {
                        range: node.range(),
                        markers: points,
                    });
                } else {
                    for child in node.children() {
                        visit(child, result);
                    }
                }
            }
            let parsed = typst_syntax::Source::detached(source);
            let mut result = Vec::new();
            visit(typst_syntax::LinkedNode::new(parsed.root()), &mut result);
            result
        }
        DocumentKind::Tex => tex_blocks(source),
        _ => Vec::new(),
    }
}

pub(crate) fn math_environment(name: &str) -> bool {
    matches!(
        name.trim_end_matches('*'),
        "math"
            | "displaymath"
            | "equation"
            | "align"
            | "alignat"
            | "flalign"
            | "eqnarray"
            | "gather"
            | "multline"
            | "aligned"
            | "alignedat"
            | "gathered"
            | "split"
            | "cases"
            | "dcases"
            | "rcases"
            | "array"
            | "matrix"
            | "pmatrix"
            | "bmatrix"
            | "Bmatrix"
            | "vmatrix"
            | "Vmatrix"
            | "smallmatrix"
    )
}

// Tolerant structural reader: no macro expansion; comments, escaped symbols
// and verbatim payloads cannot create math blocks or alignment points.
fn tex_blocks(source: &str) -> Vec<MathBlock> {
    let mut result = Vec::new();
    let mut stack: Vec<(usize, String, Vec<usize>)> = Vec::new();
    let mut i = 0;
    while i < source.len() {
        let rest = &source[i..];
        if rest.starts_with('%') {
            i += rest.find('\n').unwrap_or(rest.len());
            continue;
        }
        let (length, opening, closing) = if (rest.starts_with("\\begin")
            && !rest[6..].starts_with(|c: char| c.is_ascii_alphabetic()))
            || (rest.starts_with("\\end")
                && !rest[4..].starts_with(|c: char| c.is_ascii_alphabetic()))
        {
            let begin = rest.starts_with("\\begin");
            let command_end = if begin { 6 } else { 4 };
            let argument =
                command_end + rest[command_end..].len() - rest[command_end..].trim_start().len();
            let prefix = argument + 1;
            if rest[argument..].starts_with('{')
                && let Some(end) = rest[prefix..].find('}')
            {
                let name = &rest[prefix..prefix + end];
                let length = prefix + end + 1;
                if begin && matches!(name, "verbatim" | "verbatim*" | "lstlisting" | "minted") {
                    let close = format!("\\end{{{name}}}");
                    i += length;
                    i += source[i..]
                        .find(&close)
                        .map_or(source.len() - i, |at| at + close.len());
                    continue;
                }
                let key = math_environment(name).then(|| name.to_owned());
                (
                    length,
                    if begin { key.clone() } else { None },
                    if begin { None } else { key },
                )
            } else {
                (1, None, None)
            }
        } else if rest.starts_with("\\verb")
            && !rest[5..].starts_with(|c: char| c.is_ascii_alphabetic())
        {
            i += 5;
            if source[i..].starts_with('*') {
                i += 1;
            }
            if let Some(delimiter) = source[i..].chars().next() {
                // An incomplete \verb ends at the source line break. Treating
                // that break as its delimiter hides real math on the next line.
                if matches!(delimiter, '\n' | '\r') {
                    continue;
                }
                i += delimiter.len_utf8();
                i += source[i..]
                    .find([delimiter, '\n'])
                    .unwrap_or(source.len() - i);
                if source[i..].starts_with(delimiter) {
                    i += delimiter.len_utf8();
                }
            }
            continue;
        } else if rest.starts_with("\\[") || rest.starts_with("\\(") {
            (2, Some(rest[..2].into()), None)
        } else if rest.starts_with("\\]") || rest.starts_with("\\)") {
            (
                2,
                None,
                Some(
                    if rest.starts_with("\\]") {
                        "\\["
                    } else {
                        "\\("
                    }
                    .into(),
                ),
            )
        } else if rest.starts_with('$') {
            let length = if rest.starts_with("$$") { 2 } else { 1 };
            let key = rest[..length].to_owned();
            if stack.last().is_some_and(|(_, active, _)| *active == key) {
                (length, None, Some(key))
            } else {
                (length, Some(key), None)
            }
        } else if rest.starts_with('\\') {
            i += 1;
            if let Some(c) = source[i..].chars().next() {
                i += c.len_utf8();
            }
            continue;
        } else {
            if rest.starts_with('&') && !stack.is_empty() {
                stack.last_mut().unwrap().2.push(i);
            }
            (rest.chars().next().unwrap().len_utf8(), None, None)
        };
        if let Some(key) = opening {
            stack.push((i, key, Vec::new()));
        }
        if let Some(key) = closing
            && let Some(index) = stack.iter().rposition(|(_, active, _)| *active == key)
        {
            for (start, _, markers) in stack.drain(index..).rev() {
                result.push(MathBlock {
                    range: start..i + length,
                    markers,
                });
            }
        }
        i += length;
    }
    for (start, _, markers) in stack {
        result.push(MathBlock {
            range: start..source.len(),
            markers,
        });
    }
    result.sort_by_key(|block| (block.range.start, std::cmp::Reverse(block.range.end)));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typst_align_points_are_syntax_not_escaped_text_or_strings() {
        let source = "text \\&\n$ a & = b \\ c & = d $\n#let text = \"$ & $\"";
        let regions = blocks(source, DocumentKind::Typst);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].markers.len(), 2);
    }
    #[test]
    fn incomplete_verb_cannot_hide_math_on_the_next_line() {
        for newline in ["\n", "\r\n"] {
            let source = format!("\\verb{newline}\\[ x & y \\]{newline}Actual prose.");
            let regions = blocks(&source, DocumentKind::Tex);
            assert_eq!(regions.len(), 1, "{source:?}: {regions:?}");
            let start = source.find("\\[").unwrap();
            let end = source.find("\\]").unwrap() + 2;
            assert_eq!(regions[0].range, start..end);
            assert_eq!(regions[0].markers, vec![source.find('&').unwrap()]);
        }
    }

    #[test]
    fn tex_math_delimiters_environments_and_escaped_markers() {
        let source = "% $ &\n\\verb|$ &|\n\\begin{verbatim}\n\\begin{align}x&y\\end{align}\n\\end{verbatim}\n\\begin{align*}\na \\& b & = c \\\\\n\\begin{pmatrix}a&b\\end{pmatrix}\n\\end{align*}\n\\[ x & y \\]\n$$x&y$$\n\\(x&y\\)";
        let regions = blocks(source, DocumentKind::Tex);
        assert_eq!(regions.len(), 5);
        assert_eq!(
            regions.iter().map(|r| r.markers.len()).collect::<Vec<_>>(),
            vec![1, 1, 1, 1, 1]
        );
    }
}
