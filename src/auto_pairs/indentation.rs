//! Bounded lexical prediction for explicit Return. No AST, filesystem or service
//! request is needed; normal typing, paste and snippet expansion do not enter it.
use crate::document::DocumentKind;

const CONTEXT_BYTES: usize = 32 * 1024;
const MAX_DEPTH: usize = 128;

pub(super) struct Newline {
    pub(super) text: String,
    pub(super) advance: usize,
    pub(super) trim_after: usize,
    #[cfg(test)]
    indentation_bytes_scanned: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Close<'a> {
    Bracket(u8),
    Dollar(usize),
    TexMath(u8),
    Environment(&'a str),
}
#[derive(Clone, Copy)]
struct Frame<'a> {
    close: Close<'a>,
    indent: &'a str,
    opened_end: usize,
    body: bool,
    code: bool,
    math: bool,
    alignment: bool,
}

fn indentation(text: &str) -> &str {
    &text[..text
        .bytes()
        .take_while(|ch| matches!(ch, b' ' | b'\t'))
        .count()]
}
pub(super) fn line_ending(source: &str, byte: usize, start: usize) -> &'static str {
    let end = source.floor_char_boundary(source.len().min(byte.saturating_add(CONTEXT_BYTES)));
    if let Some((line, _)) = source[byte..end].split_once('\n') {
        if line.ends_with('\r') { "\r\n" } else { "\n" }
    } else if source[..start].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}
fn context_start(source: &str, byte: usize) -> usize {
    let start = source.ceil_char_boundary(byte.saturating_sub(CONTEXT_BYTES));
    if start == 0 {
        0
    } else {
        // Never start lexing halfway through a UTF-8 character or physical line.
        source[start..byte]
            .find('\n')
            .map_or(byte, |at| start + at + 1)
    }
}
fn environment(source: &str, at: usize, end: usize, begin: bool) -> Option<(&str, usize)> {
    let command = if begin { "\\begin" } else { "\\end" };
    let mut next = at + command.len();
    if !source[at..end].starts_with(command) {
        return None;
    }
    loop {
        while source
            .as_bytes()
            .get(next)
            .is_some_and(u8::is_ascii_whitespace)
            && next < end
        {
            next += 1;
        }
        if source.as_bytes().get(next) != Some(&b'%') || next >= end {
            break;
        }
        next += source[next..end].find('\n').unwrap_or(end - next);
    }
    if next >= end || source.as_bytes()[next] != b'{' {
        return None;
    }
    let name_start = next + 1;
    next = name_start + source[name_start..end].find('}')?;
    let name = source[name_start..next].trim();
    (!name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'*' | b'_' | b'-' | b'@')))
    .then_some((name, next + 1))
}
fn raw_environment(name: &str) -> bool {
    matches!(
        name.trim_end_matches('*'),
        "verbatim" | "Verbatim" | "lstlisting" | "minted" | "comment"
    )
}
fn pop<'a>(stack: &mut Vec<Frame<'a>>, close: Close<'a>) -> Option<Frame<'a>> {
    let index = stack.iter().rposition(|frame| frame.close == close)?;
    let frame = stack[index];
    stack.truncate(index);
    Some(frame)
}

fn typst_code(stack: &[Frame<'_>], last_hash: Option<usize>) -> bool {
    let frame = stack.last();
    frame.is_some_and(|frame| frame.code || frame.math)
        || last_hash.is_some_and(|hash| frame.is_none_or(|frame| hash >= frame.opened_end))
}

fn crossed_lines(
    source: &str,
    from: usize,
    to: usize,
    line: &mut usize,
    last_hash: &mut Option<usize>,
) {
    if let Some(at) = source[from..to].rfind('\n') {
        *line = from + at + 1;
        *last_hash = None;
    }
}

pub(super) fn newline(source: &str, byte: usize, kind: DocumentKind, spaces: u8) -> Newline {
    let start = source[..byte].rfind('\n').map_or(0, |at| at + 1);
    let prefix = &source[start..byte];
    let base = indentation(prefix);
    let ending = line_ending(source, byte, start);
    let unit = if spaces == 0 {
        "\t".into()
    } else {
        " ".repeat(usize::from(spaces.min(16)))
    };
    let mut desired = base.to_owned();
    let mut closing_indent = None;
    let mut trim_after = 0;
    let mut cached_indent = (usize::MAX, "");
    #[cfg(test)]
    let mut indentation_bytes_scanned = 0;
    if !prefix.trim().is_empty() && matches!(kind, DocumentKind::Typst | DocumentKind::Tex) {
        let mut at = context_start(source, byte);
        let mut line_start = at;
        let mut last_hash = None;
        let mut stack = Vec::new();
        let mut last_close = None;
        let mut literal = false;
        let mut overflow = false;
        while at < byte {
            let ch = source.as_bytes()[at];
            if ch == b'\n' {
                line_start = at + 1;
                last_hash = None;
                last_close = None;
                at += 1;
                continue;
            }
            if ch.is_ascii_whitespace() {
                at += 1;
                continue;
            }
            if kind == DocumentKind::Typst {
                if source[at..byte].starts_with("//") {
                    at += source[at..byte].find('\n').unwrap_or(byte - at);
                    continue;
                }
                if source[at..byte].starts_with("/*") {
                    let from = at;
                    let mut depth = 1usize;
                    at += 2;
                    while at < byte && depth > 0 {
                        if source[at..byte].starts_with("/*") {
                            depth += 1;
                            at += 2;
                        } else if source[at..byte].starts_with("*/") {
                            depth -= 1;
                            at += 2;
                        } else {
                            at += source[at..].chars().next().unwrap().len_utf8();
                        }
                    }
                    if depth > 0 {
                        literal = true;
                        break;
                    }
                    crossed_lines(source, from, at, &mut line_start, &mut last_hash);
                    continue;
                }
                if ch == b'"' && typst_code(&stack, last_hash) {
                    let from = at;
                    at += 1;
                    let mut closed = false;
                    while at < byte {
                        let next = source.as_bytes()[at];
                        at += source[at..].chars().next().unwrap().len_utf8();
                        if next == b'\\' && at < byte {
                            at += source[at..].chars().next().unwrap().len_utf8();
                        } else if next == b'"' {
                            closed = true;
                            break;
                        }
                    }
                    if !closed {
                        literal = true;
                        break;
                    }
                    crossed_lines(source, from, at, &mut line_start, &mut last_hash);
                    last_close = None;
                    continue;
                }
                if ch == b'`' {
                    let from = at;
                    let ticks = source[at..byte]
                        .bytes()
                        .take_while(|ch| *ch == b'`')
                        .count();
                    let delimiter = &source[at..at + ticks];
                    at += ticks;
                    if let Some(end) = source[at..byte].find(delimiter) {
                        at += end + ticks;
                    } else {
                        literal = true;
                        break;
                    }
                    crossed_lines(source, from, at, &mut line_start, &mut last_hash);
                    last_close = None;
                    continue;
                }
                if ch == b'#' {
                    last_hash = Some(at);
                }
            } else if ch == b'%' {
                at += source[at..byte].find('\n').unwrap_or(byte - at);
                continue;
            }
            if cached_indent.0 != line_start {
                cached_indent = (line_start, indentation(&source[line_start..byte]));
                #[cfg(test)]
                {
                    indentation_bytes_scanned += cached_indent.1.len() + 1;
                }
            }
            let indent = cached_indent.1;
            if ch == b'\\' {
                if kind == DocumentKind::Tex {
                    if let Some((name, end)) = environment(source, at, byte, true) {
                        if raw_environment(name) {
                            let marker = format!("\\end{{{name}}}");
                            if let Some(close) = source[end..byte].find(&marker) {
                                let to = end + close + marker.len();
                                crossed_lines(source, at, to, &mut line_start, &mut last_hash);
                                at = to;
                                continue;
                            }
                            literal = true;
                            break;
                        }
                        let math = crate::editor_math_guides::math_environment(name);
                        if stack.len() >= MAX_DEPTH {
                            overflow = true;
                            break;
                        }
                        stack.push(Frame {
                            close: Close::Environment(name),
                            indent,
                            opened_end: end,
                            body: name != "document",
                            code: false,
                            math,
                            alignment: false,
                        });
                        crossed_lines(source, at, end, &mut line_start, &mut last_hash);
                        at = end;
                        last_close = None;
                        continue;
                    }
                    if let Some((name, end)) = environment(source, at, byte, false) {
                        last_close = pop(&mut stack, Close::Environment(name));
                        crossed_lines(source, at, end, &mut line_start, &mut last_hash);
                        at = end;
                        continue;
                    }
                    if let Some(end) =
                        crate::editor_math_guides::tex_inline_literal_end(&source[..byte], at)
                    {
                        crossed_lines(source, at, end, &mut line_start, &mut last_hash);
                        at = end;
                        last_close = None;
                        continue;
                    }
                    if let Some(next @ (b'[' | b']' | b'(' | b')')) = source
                        .as_bytes()
                        .get(at + 1)
                        .copied()
                        .filter(|_| at + 1 < byte)
                    {
                        if matches!(next, b'[' | b'(') {
                            if stack.len() >= MAX_DEPTH {
                                overflow = true;
                                break;
                            }
                            stack.push(Frame {
                                close: Close::TexMath(if next == b'[' { b']' } else { b')' }),
                                indent,
                                opened_end: at + 2,
                                body: next == b'[',
                                code: false,
                                math: true,
                                alignment: false,
                            });
                            last_close = None;
                        } else {
                            last_close = pop(&mut stack, Close::TexMath(next));
                        }
                        at += 2;
                        continue;
                    }
                    // Escaped punctuation and control words are not group tokens.
                    at += 1;
                    if at < byte && source.as_bytes()[at].is_ascii_alphabetic() {
                        while at < byte && source.as_bytes()[at].is_ascii_alphabetic() {
                            at += 1;
                        }
                    } else if at < byte {
                        at += source[at..].chars().next().unwrap().len_utf8();
                    }
                    last_close = None;
                    continue;
                }
                at += 1;
                if at < byte {
                    at += source[at..].chars().next().unwrap().len_utf8();
                }
                last_close = None;
                continue;
            }
            if ch == b'$' {
                let count = if kind == DocumentKind::Tex {
                    source[at..byte]
                        .bytes()
                        .take(2)
                        .take_while(|ch| *ch == b'$')
                        .count()
                } else {
                    1
                };
                if stack
                    .iter()
                    .any(|frame| frame.close == Close::Dollar(count))
                {
                    last_close = pop(&mut stack, Close::Dollar(count));
                } else {
                    if stack.len() >= MAX_DEPTH {
                        overflow = true;
                        break;
                    }
                    let display = if kind == DocumentKind::Tex {
                        count == 2
                    } else {
                        source[line_start..at].trim().is_empty()
                            && source[at + 1..]
                                .chars()
                                .next()
                                .is_none_or(char::is_whitespace)
                    };
                    stack.push(Frame {
                        close: Close::Dollar(count),
                        indent,
                        opened_end: at + count,
                        body: display,
                        code: false,
                        math: true,
                        alignment: false,
                    });
                    last_close = None;
                }
                at += count;
                continue;
            }
            if ch == b'&'
                && let Some(frame) = stack
                    .iter_mut()
                    .rev()
                    .find(|frame| frame.math && !matches!(frame.close, Close::Bracket(_)))
            {
                frame.alignment = true;
            }
            let math = stack.last().is_some_and(|frame| frame.math);
            // Content blocks suspend an outer code escape. A fresh # inside the
            // block activates code again; closing content restores its parent.
            let structural = kind == DocumentKind::Tex || typst_code(&stack, last_hash);
            let content_bracket = kind == DocumentKind::Typst
                && matches!(ch, b'[' | b']')
                && stack.last().is_some_and(|frame| {
                    frame.close == Close::Bracket(b']') && !frame.code && !frame.math
                });
            if (structural || content_bracket)
                && matches!(ch, b'(' | b'[' | b'{')
                && (kind != DocumentKind::Tex || ch == b'{')
            {
                if stack.len() >= MAX_DEPTH {
                    overflow = true;
                    break;
                }
                let close = match ch {
                    b'(' => b')',
                    b'[' => b']',
                    _ => b'}',
                };
                stack.push(Frame {
                    close: Close::Bracket(close),
                    indent,
                    opened_end: at + 1,
                    body: true,
                    code: ch != b'[',
                    math,
                    alignment: false,
                });
                last_close = None;
            } else if (structural || content_bracket) && matches!(ch, b')' | b']' | b'}') {
                last_close = pop(&mut stack, Close::Bracket(ch));
            } else if !ch.is_ascii_whitespace() {
                last_close = None;
            }
            at += source[at..].chars().next().unwrap().len_utf8();
        }
        if !literal && !overflow {
            if let Some(frame) = stack.last().filter(|frame| frame.opened_end > start) {
                desired = format!("{}{}", frame.indent, if frame.body { &unit } else { "" });
            } else if let Some(frame) = last_close.filter(|frame| {
                prefix.trim_start().starts_with([')', ']', '}'])
                    || prefix.trim_start().starts_with("\\end")
                    || prefix.trim_start().starts_with("\\]")
                    || (frame.body
                        && matches!(frame.close, Close::Dollar(_))
                        && prefix.trim_start().starts_with('$'))
            }) {
                desired = frame.indent.into();
            }
            let row_break = if kind == DocumentKind::Tex {
                prefix.trim_end().ends_with("\\\\")
            } else {
                prefix.trim_end().ends_with('\\')
            };
            if row_break
                && let Some(frame) = stack
                    .iter()
                    .rev()
                    .find(|frame| frame.math && frame.alignment && frame.body)
            {
                desired = format!("{}{}", frame.indent, unit);
            }
            let rest = &source[byte..source
                .floor_char_boundary(source.len().min(byte.saturating_add(CONTEXT_BYTES)))];
            let whitespace = indentation(rest).len();
            let rest = &rest[whitespace..];
            if let Some(frame) = stack.last() {
                let matches = match frame.close {
                    Close::Bracket(close) => rest.as_bytes().first() == Some(&close),
                    Close::Dollar(count) => rest.starts_with(&"$".repeat(count)),
                    Close::TexMath(close) => rest.as_bytes().starts_with(&[b'\\', close]),
                    Close::Environment(name) => environment(rest, 0, rest.len(), false)
                        .is_some_and(|(closed, _)| name == closed),
                };
                if matches && (frame.body || matches!(frame.close, Close::Environment(_))) {
                    closing_indent = Some(frame.indent);
                    trim_after = whitespace;
                }
            }
        }
    }
    let text = format!("{ending}{desired}");
    let advance = text.len(); // Newline and indentation contain ASCII only.
    let text = if let Some(indent) = closing_indent {
        format!("{text}{ending}{indent}")
    } else {
        text
    };
    Newline {
        text,
        advance,
        trim_after,
        #[cfg(test)]
        indentation_bytes_scanned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn enter(before: &str, kind: DocumentKind, spaces: u8) -> String {
        let byte = before.find('|').unwrap();
        let mut source = before.replacen('|', "", 1);
        let edit = newline(&source, byte, kind, spaces);
        source.replace_range(byte..byte + edit.trim_after, &edit.text);
        source.insert(byte + edit.advance, '|');
        source
    }

    #[test]
    fn typst_structural_blocks_arrays_and_call_closers_match_formatter_indentation() {
        for (before, after) in [
            ("#let f() = {|}", "#let f() = {\n  |\n}"),
            ("#let xs = (|)", "#let xs = (\n  |\n)"),
            ("#emph[|]", "#emph[\n  |\n]"),
            ("#table(\n  columns: 2,|)", "#table(\n  columns: 2,\n  |\n)"),
            ("#table(\n  [A],|  )", "#table(\n  [A],\n  |\n)"),
            (
                "#let f() = { // explanation|",
                "#let f() = { // explanation\n  |",
            ),
            (
                "#let f() = {\n  value\n  }|",
                "#let f() = {\n  value\n  }\n|",
            ),
            ("  words (|", "  words (\n  |"),
            (
                "  quoted prose \"here #let f() = {|}",
                "  quoted prose \"here #let f() = {\n    |\n  }",
            ),
        ] {
            assert_eq!(enter(before, DocumentKind::Typst, 2), after, "{before}");
        }
    }

    #[test]
    fn typst_content_blocks_scope_code_escapes_and_restore_parent_code() {
        for (before, after) in [
            ("#emph[prose (|)]", "#emph[prose (\n  |)]"),
            ("#emph[quoted \"prose (|)]", "#emph[quoted \"prose (\n  |)]"),
            ("#emph[#let f() = {|}]", "#emph[#let f() = {\n  |\n}]"),
            (
                "#emph[#let content = [nested [text]]; {|}]",
                "#emph[#let content = [nested [text]]; {\n  |\n}]",
            ),
            (
                "#let xs = ([outer [inner]], (|))",
                "#let xs = ([outer [inner]], (\n  |\n))",
            ),
        ] {
            assert_eq!(enter(before, DocumentKind::Typst, 2), after, "{before}");
        }
    }

    #[test]
    fn tex_environment_headers_groups_display_math_and_document_exemption_match_formatters() {
        for (before, after) in [
            ("\\begin{document}|", "\\begin{document}\n|"),
            (
                "\\begin{document}|\\end{document}",
                "\\begin{document}\n|\n\\end{document}",
            ),
            ("\\begin{align}|", "\\begin{align}\n  |"),
            ("  \\begin{equation*}|", "  \\begin{equation*}\n    |"),
            ("\\begin\n  {align}|", "\\begin\n  {align}\n  |"),
            ("\\begin{tabular}{ll}|", "\\begin{tabular}{ll}\n  |"),
            ("\\begin{itemize}|", "\\begin{itemize}\n  |"),
            ("\\newcommand{\\f}[1]{|}", "\\newcommand{\\f}[1]{\n  |\n}"),
            ("\\[|", "\\[\n  |"),
            ("$$|", "$$\n  |"),
            (
                "  \\begin{align}\n    x \\\\\n    \\end{align}|",
                "  \\begin{align}\n    x \\\\\n    \\end{align}\n  |",
            ),
            ("\\beginning{align}|", "\\beginning{align}\n|"),
        ] {
            assert_eq!(enter(before, DocumentKind::Tex, 2), after, "{before}");
        }
    }

    #[test]
    fn strings_comments_raw_literals_and_escaped_groups_do_not_create_structure() {
        for (kind, before, after) in [
            (
                DocumentKind::Typst,
                "  // fake #let f() = {|",
                "  // fake #let f() = {\n  |",
            ),
            (
                DocumentKind::Typst,
                "/*\n  #let fake() = {|",
                "/*\n  #let fake() = {\n  |",
            ),
            (
                DocumentKind::Typst,
                "```typ\n  #let fake() = {|",
                "```typ\n  #let fake() = {\n  |",
            ),
            (
                DocumentKind::Typst,
                "#let text = \"literal {|",
                "#let text = \"literal {\n|",
            ),
            (
                DocumentKind::Typst,
                "#let text = \"literal {\"|",
                "#let text = \"literal {\"\n|",
            ),
            (
                DocumentKind::Typst,
                "#let text = \"one\n  two\"\n    #let f() = {|}",
                "#let text = \"one\n  two\"\n    #let f() = {\n      |\n    }",
            ),
            (
                DocumentKind::Tex,
                "  % fake \\begin{align} {|",
                "  % fake \\begin{align} {\n  |",
            ),
            (DocumentKind::Tex, "  \\verb!{!|", "  \\verb!{!\n  |"),
            (DocumentKind::Tex, "  \\{|", "  \\{\n  |"),
            (
                DocumentKind::Tex,
                "\\begin{verbatim}\n  \\begin{align} {|",
                "\\begin{verbatim}\n  \\begin{align} {\n  |",
            ),
        ] {
            assert_eq!(enter(before, kind, 2), after, "{before}");
        }
    }

    #[test]
    fn aligned_rows_return_to_math_body_instead_of_copying_lhs_padding() {
        assert_eq!(
            enter("$\n      x & = y \\|\n$", DocumentKind::Typst, 2),
            "$\n      x & = y \\\n  |\n$"
        );
        assert_eq!(
            enter(
                "  \\begin{align}\n          x & = y \\\\|\n  \\end{align}",
                DocumentKind::Tex,
                2
            ),
            "  \\begin{align}\n          x & = y \\\\\n    |\n  \\end{align}"
        );
        // A continuation without an explicit row break retains authored nesting.
        assert_eq!(
            enter("$\n  x &= y\n    + z|\n$", DocumentKind::Typst, 2),
            "$\n  x &= y\n    + z\n    |\n$"
        );
    }

    #[test]
    fn exact_blank_indentation_configured_units_and_local_crlf_are_preserved() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            assert_eq!(enter("\t  |", kind, 2), "\t  \n\t  |");
            assert_eq!(enter("\t  text|", kind, 0), "\t  text\n\t  |");
            assert_eq!(
                enter("first\r\n\t  text|", kind, 2),
                "first\r\n\t  text\r\n\t  |"
            );
            assert_eq!(enter("  text|\r\nnext", kind, 2), "  text\r\n  |\r\nnext");
        }
        assert_eq!(
            enter(" \t#let f() = {|}", DocumentKind::Typst, 0),
            " \t#let f() = {\n \t\t|\n \t}"
        );
        assert_eq!(
            enter(" \t\\begin{align}|", DocumentKind::Tex, 0),
            " \t\\begin{align}\n \t\t|"
        );
        assert_eq!(
            enter("#let f() = {|}", DocumentKind::Typst, 4),
            "#let f() = {\n    |\n}"
        );
    }

    #[test]
    fn heavily_padded_lines_scan_indentation_once_instead_of_once_per_token() {
        let source = format!("\n{}{}", " ".repeat(16_000), "x".repeat(15_000));
        let edit = newline(&source, source.len(), DocumentKind::Typst, 2);
        assert_eq!(edit.text.len(), 16_001);
        assert!(
            edit.indentation_bytes_scanned <= 16_001,
            "{}",
            edit.indentation_bytes_scanned
        );
    }

    #[test]
    fn current_line_terminator_wins_in_mixed_ending_documents() {
        assert_eq!(enter("a\r\nb|\nc", DocumentKind::Typst, 2), "a\r\nb\n|\nc");
        assert_eq!(enter("a\nb|\r\nc", DocumentKind::Tex, 2), "a\nb\r\n|\r\nc");
        assert_eq!(
            enter("$\n  x\n  $|", DocumentKind::Typst, 2),
            "$\n  x\n  $\n|"
        );
        assert_eq!(
            enter("$$\n  x\n  $$|", DocumentKind::Tex, 2),
            "$$\n  x\n  $$\n|"
        );
    }

    #[test]
    fn multiline_inline_literal_headers_keep_the_following_line_origin() {
        assert_eq!(
            enter(
                "\\mintinline\n{tex}{text} \\begin{align}|",
                DocumentKind::Tex,
                2
            ),
            "\\mintinline\n{tex}{text} \\begin{align}\n  |"
        );
        assert_eq!(
            enter(
                "\\mintinline{tex}{one\n    two} \\begin{align}|",
                DocumentKind::Tex,
                2
            ),
            "\\mintinline{tex}{one\n    two} \\begin{align}\n      |"
        );
    }

    #[test]
    fn lexical_work_and_depth_are_bounded_and_unicode_lookahead_is_safe() {
        let source = format!("{}\n#let f() = {{", "prose\n".repeat(100_000));
        assert!(source.len() - context_start(&source, source.len()) <= CONTEXT_BYTES);
        assert_eq!(
            newline(&source, source.len(), DocumentKind::Typst, 2).text,
            "\n  "
        );
        let suffix = format!("a{}", "😀".repeat(CONTEXT_BYTES));
        assert_eq!(newline(&suffix, 1, DocumentKind::Typst, 2).text, "\n");
        let source = format!("#let x = {}", "(".repeat(MAX_DEPTH + 1));
        assert_eq!(
            newline(&source, source.len(), DocumentKind::Typst, 2).text,
            "\n"
        );
        // Without a visible structural opener, the bounded fallback is literal
        // indentation copying, including inside very long unfinished literals.
        let source = format!("```typ\n{}\n    text", "raw text\n".repeat(10_000));
        assert_eq!(
            newline(&source, source.len(), DocumentKind::Typst, 2).text,
            "\n    "
        );
    }
}
