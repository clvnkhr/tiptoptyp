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

/// End of a literal TeX code command, retaining its original byte offsets.
/// Unterminated delimiter forms stop at the line break, never at a later match.
pub(crate) fn tex_inline_literal_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(start) != Some(&b'\\') {
        return None;
    }
    let mut at = start + 1;
    while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
        at += 1;
    }
    let name = source.get(start + 1..at)?;
    if !matches!(name, "verb" | "lstinline" | "mintinline") {
        return None;
    }
    fn header_space(source: &str, mut at: usize, skip_comments: bool) -> usize {
        loop {
            while let Some(ch) = source[at..].chars().next().filter(|ch| ch.is_whitespace()) {
                at += ch.len_utf8();
            }
            if skip_comments && source[at..].starts_with('%') {
                at += source[at..].find(['\n', '\r']).unwrap_or(source.len() - at);
            } else {
                return at;
            }
        }
    }
    fn group_end(source: &str, mut at: usize, open: u8, close: u8) -> (usize, bool) {
        at += 1;
        let mut depth = 1_usize;
        let mut braces = 0_usize;
        while at < source.len() {
            let byte = source.as_bytes()[at];
            if matches!(byte, b'\n' | b'\r') && open == b'[' {
                return (at, false);
            }
            if byte == b'\\' {
                at += 1;
                at += source[at..].chars().next().map_or(0, char::len_utf8);
                continue;
            }
            if open == b'[' && byte == b'{' {
                braces += 1;
            } else if open == b'[' && byte == b'}' {
                braces = braces.saturating_sub(1);
            } else if braces == 0 && byte == open {
                depth += 1;
            } else if braces == 0 && byte == close {
                depth -= 1;
                if depth == 0 {
                    return (at + 1, true);
                }
            }
            at += source[at..].chars().next().unwrap().len_utf8();
        }
        (at, false)
    }
    if name == "verb" {
        if bytes.get(at) == Some(&b'*') {
            at += 1;
        }
    } else {
        at = header_space(source, at, true);
        if bytes.get(at) == Some(&b'[') {
            let (end, closed) = group_end(source, at, b'[', b']');
            if !closed {
                return Some(end);
            }
            at = header_space(source, end, true);
        }
        if name == "mintinline" {
            if bytes.get(at) != Some(&b'{') {
                return Some(at);
            }
            let (end, closed) = group_end(source, at, b'{', b'}');
            if !closed {
                return Some(end);
            }
            // The payload reader has made percent a literal delimiter already.
            at = header_space(source, end, false);
        }
        if bytes.get(at) == Some(&b'{') {
            return Some(group_end(source, at, b'{', b'}').0);
        }
    }
    if let Some(delimiter) = source[at..].chars().next()
        && !matches!(delimiter, '\n' | '\r')
    {
        at += delimiter.len_utf8();
        at += source[at..]
            .find([delimiter, '\n', '\r'])
            .unwrap_or(source.len() - at);
        if source[at..].starts_with(delimiter) {
            at += delimiter.len_utf8();
        }
    }
    Some(at)
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
        } else if let Some(end) = tex_inline_literal_end(source, i) {
            i = end;
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
    fn inline_tex_code_cannot_create_math_blocks_or_alignment_markers() {
        for code in [
            r"\verb|$ & \[fake\]|",
            r"\lstinline|$ & \[fake\]|",
            r"\lstinline*$ & \[fake\]*",
            r"\lstinline[language={[LaTeX]TeX}]{$ & \text{fake}}",
            r"\lstinline[language=TeX]!$ & \[fake\]!",
            r"\lstinline§$ & \[fake\]§",
            r"\mintinline{tex}|$ & \[fake\]|",
            r"\mintinline{tex}%$ & \[fake\]%",
            "\\mintinline% header\n{tex}|$ & \\[fake\\]|",
            r"\mintinline[breaklines]{tex}{$ & \text{fake}}",
            "\\lstinline|$ &\n",
            "\\mintinline{tex}|$ &\r\n",
        ] {
            let real = r"\[ x & y \]";
            let source = format!("é {code}\n{real}\nActual prose.");
            let regions = blocks(&source, DocumentKind::Tex);
            assert_eq!(regions.len(), 1, "{source:?}: {regions:?}");
            assert_eq!(&source[regions[0].range.clone()], real, "{source:?}");
            assert_eq!(regions[0].markers.len(), 1, "{source:?}");
        }
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
