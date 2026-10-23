//! Provider-neutral diagnostic exclusions, expressed only by standalone comments.
//! `ttt: on` starts an exclusion; `ttt: off` restores normal diagnostics.
use crate::document::DocumentKind;
use std::ops::Range;
use typst_syntax::{Source, SyntaxKind, SyntaxNode};

/// Sorted, non-overlapping, half-open ranges of one-based source lines.
/// Cache this value with the source revision, not with any individual provider.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Suppression {
    lines: Vec<Range<usize>>,
}

impl Suppression {
    pub(crate) fn parse(source: &str, kind: DocumentKind) -> Self {
        // Most documents have no directives. Avoid syntax work and allocations
        // in that common case; mere mentions still need lexical validation.
        if !source.contains("ttt:") {
            return Self::default();
        }
        let comments = match kind {
            DocumentKind::Typst => typst_comments(source),
            DocumentKind::Tex => tex_comments(source),
            _ => return Self::default(),
        };
        let mut lines = Vec::new();
        let mut excluded_from = None;
        let mut line = 1;
        let mut previous = 0;
        for comment in comments {
            line += source[previous..comment.start]
                .bytes()
                .filter(|&byte| byte == b'\n')
                .count();
            previous = comment.start;
            let line_start = source[..comment.start].rfind('\n').map_or(0, |at| at + 1);
            if !source[line_start..comment.start].trim().is_empty() {
                continue;
            }
            let prefix = if kind == DocumentKind::Typst { 2 } else { 1 };
            let Some(switch) = source[comment.start + prefix..comment.end]
                .trim()
                .strip_prefix("ttt:")
                .map(str::trim)
            else {
                continue;
            };
            match switch {
                "on" => {
                    excluded_from.get_or_insert(line + 1);
                }
                "off" => {
                    if let Some(start) = excluded_from.take()
                        && start < line
                    {
                        lines.push(start..line);
                    }
                }
                _ => {}
            }
        }
        if let Some(start) = excluded_from {
            let end = line
                + source[previous..]
                    .bytes()
                    .filter(|&byte| byte == b'\n')
                    .count()
                + 1;
            if start < end {
                lines.push(start..end);
            }
        }
        Self { lines }
    }

    pub(crate) fn contains_line(&self, line: usize) -> bool {
        let after = self.lines.partition_point(|range| range.start <= line);
        after > 0 && self.lines[after - 1].contains(&line)
    }
}

fn typst_comments(source: &str) -> Vec<Range<usize>> {
    fn visit(node: &SyntaxNode, offset: usize, comments: &mut Vec<Range<usize>>) {
        match node.kind() {
            SyntaxKind::LineComment => comments.push(offset..offset + node.len()),
            SyntaxKind::Str | SyntaxKind::Raw | SyntaxKind::BlockComment => {}
            _ => {
                let mut offset = offset;
                for child in node.children() {
                    visit(child, offset, comments);
                    offset += child.len();
                }
            }
        }
    }
    let parsed = Source::detached(source);
    let mut comments = Vec::new();
    visit(parsed.root(), 0, &mut comments);
    comments
}

/// Tolerant TeX lexical reading, without executing macros or changing catcodes.
/// Shared inline-literal recognition keeps samples from creating directives.
fn tex_comments(source: &str) -> Vec<Range<usize>> {
    let mut comments = Vec::new();
    let mut at = 0;
    while at < source.len() {
        match source.as_bytes()[at] {
            b'%' => {
                let end = at + source[at..].find('\n').unwrap_or(source.len() - at);
                comments.push(at..end);
                at = end;
            }
            b'\\' => {
                if let Some(literal) = crate::editor_math_guides::tex_inline_literal(source, at) {
                    // Header whitespace/options/language arguments still use
                    // ordinary TeX comments; only the literal payload changes
                    // their meaning. Keep extraction within the shared span.
                    tex_inline_header_comments(&source[..literal.end], at, &mut comments);
                    at = literal.end;
                    continue;
                }
                let start = at + 1;
                at = start;
                while source
                    .as_bytes()
                    .get(at)
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'@')
                {
                    at += 1;
                }
                if at == start {
                    at += source[at..].chars().next().map_or(0, char::len_utf8);
                    continue;
                }
                if &source[start..at] == "begin" {
                    let argument = tex_header_space(source, at, &mut comments);
                    if source[argument..].starts_with('{')
                        && let Some(end) =
                            source[argument + 1..].find(['}', '{', '\\', '%', '\n', '\r'])
                    {
                        let end = argument + 1 + end;
                        if source.as_bytes()[end] == b'}' {
                            let environment = source[argument + 1..end].trim();
                            if matches!(
                                environment,
                                "verbatim"
                                    | "verbatim*"
                                    | "Verbatim"
                                    | "BVerbatim"
                                    | "LVerbatim"
                                    | "SaveVerbatim"
                                    | "VerbatimOut"
                                    | "lstlisting"
                                    | "minted"
                                    | "comment"
                                    | "filecontents"
                                    | "filecontents*"
                            ) {
                                at = end + 1;
                                if environment == "comment" {
                                    at = tex_comment_environment_end(source, at);
                                } else {
                                    let closing = format!("\\end{{{environment}}}");
                                    at += source[at..]
                                        .find(&closing)
                                        .map_or(source.len() - at, |offset| offset + closing.len());
                                }
                            } else {
                                at = end + 1;
                            }
                        } else {
                            at = argument;
                        }
                    } else {
                        at = argument;
                    }
                }
            }
            _ => at += source[at..].chars().next().unwrap().len_utf8(),
        }
    }
    comments
}

/// comment.sty's ProcessCommentLine compares a whole line to EndCommentTest.
/// An inline example, indented marker or trailing text cannot end its payload.
fn tex_comment_environment_end(source: &str, start: usize) -> usize {
    let closing = "\\end{comment}";
    // ProcessComment discards the remainder of the opening command's line.
    let mut at = start
        + source[start..]
            .find('\n')
            .map_or(source.len() - start, |offset| offset + 1);
    while at < source.len() {
        let end = at + source[at..].find('\n').unwrap_or(source.len() - at);
        let line = source[at..end]
            .strip_suffix('\r')
            .unwrap_or(&source[at..end]);
        if line == closing {
            return at + closing.len();
        }
        at = (end + 1).min(source.len());
    }
    source.len()
}

fn tex_inline_header_comments(source: &str, start: usize, comments: &mut Vec<Range<usize>>) {
    let mut at = start + 1;
    while source
        .as_bytes()
        .get(at)
        .is_some_and(u8::is_ascii_alphabetic)
    {
        at += 1;
    }
    let name = &source[start + 1..at];
    if !matches!(name, "lstinline" | "mintinline") {
        return;
    }
    at = tex_header_space(source, at, comments);
    if source[at..].starts_with('[') {
        let Some(end) = tex_header_group_end(source, at, b'[', b']', comments) else {
            return;
        };
        at = tex_header_space(source, end, comments);
    }
    if name == "mintinline" && source[at..].starts_with('{') {
        let _ = tex_header_group_end(source, at, b'{', b'}', comments);
        // After the language argument, '%' may itself be the literal
        // delimiter. Do not continue searching for comments into the payload.
    }
}

fn tex_header_group_end(
    source: &str,
    start: usize,
    open: u8,
    close: u8,
    comments: &mut Vec<Range<usize>>,
) -> Option<usize> {
    let mut at = start + 1;
    let mut depth = 1;
    let mut braces = 0_usize;
    while at < source.len() {
        let byte = source.as_bytes()[at];
        if byte == b'%' {
            let end = at + source[at..].find('\n').unwrap_or(source.len() - at);
            comments.push(at..end);
            at = end;
            continue;
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
                return Some(at + 1);
            }
        }
        at += source[at..].chars().next().unwrap().len_utf8();
    }
    None
}

fn tex_header_space(source: &str, mut at: usize, comments: &mut Vec<Range<usize>>) -> usize {
    loop {
        while let Some(character) = source[at..].chars().next().filter(|ch| ch.is_whitespace()) {
            at += character.len_utf8();
        }
        if source[at..].starts_with('%') {
            let end = at + source[at..].find('\n').unwrap_or(source.len() - at);
            comments.push(at..end);
            at = end;
        } else {
            return at;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suppressed(source: &str, kind: DocumentKind) -> Vec<usize> {
        let parsed = Suppression::parse(source, kind);
        (0..=source.lines().count() + 2)
            .filter(|&line| parsed.contains_line(line))
            .collect()
    }

    #[test]
    fn switches_suppress_only_between_standalone_markers_in_both_languages() {
        for (prefix, kind) in [("%", DocumentKind::Tex), ("//", DocumentKind::Typst)] {
            for newline in ["\n", "\r\n"] {
                let source = [
                    "Visible before".to_owned(),
                    format!("  {prefix}ttt: on  "),
                    "Hidden typo".into(),
                    "Hidden compile error".into(),
                    format!("\t{prefix} ttt: off\t"),
                    "Visible after".into(),
                ]
                .join(newline);
                assert_eq!(suppressed(&source, kind), [3, 4], "{source}");
            }
        }
    }

    #[test]
    fn switches_are_idempotent_support_multiple_regions_and_end_at_eof() {
        let source = "%ttt: off\nvisible\n%ttt: on\nhidden\n%ttt: on\nstill hidden\n%ttt: off\nvisible\n%ttt: on\nhidden at EOF";
        let parsed = Suppression::parse(source, DocumentKind::Tex);
        assert_eq!(parsed.lines, [4..7, 10..11]);
        assert_eq!(suppressed(source, DocumentKind::Tex), [4, 5, 6, 10]);
        for source in ["%ttt: on", "%ttt: on\n%ttt: off"] {
            assert!(
                Suppression::parse(source, DocumentKind::Tex)
                    .lines
                    .is_empty()
            );
        }
    }

    #[test]
    fn unclosed_region_includes_the_final_empty_source_line_but_not_beyond_eof() {
        let parsed = Suppression::parse("%ttt: on\nhidden\n", DocumentKind::Tex);
        assert!(!parsed.contains_line(0));
        assert!(!parsed.contains_line(1));
        assert!(parsed.contains_line(2));
        assert!(parsed.contains_line(3));
        assert!(!parsed.contains_line(4));
        assert!(!parsed.contains_line(usize::MAX));
    }

    #[test]
    fn directives_are_strict_not_prose_trailing_comments_or_unknown_switches() {
        for (prefix, kind) in [("%", DocumentKind::Tex), ("//", DocumentKind::Typst)] {
            for directive in [
                "mention ttt: on",
                "ttt: on followed by explanation",
                "TTT: on",
                "ttt: ON",
                "ttt : on",
                "ttt: enable",
                "ttt:",
            ] {
                let source = format!("{prefix} {directive}\nvisible");
                assert!(
                    Suppression::parse(&source, kind).lines.is_empty(),
                    "{source}"
                );
            }
            let source = format!("some code {prefix} ttt: on\nvisible");
            assert!(Suppression::parse(&source, kind).lines.is_empty());
        }
    }

    #[test]
    fn language_comment_markers_never_apply_to_the_other_language_or_plain_text() {
        for source in ["%ttt: on\nhidden", "//ttt: on\nhidden"] {
            for kind in [DocumentKind::Text, DocumentKind::Pdf, DocumentKind::Image] {
                assert!(Suppression::parse(source, kind).lines.is_empty());
            }
        }
        assert!(
            Suppression::parse("%ttt: on\nvisible", DocumentKind::Typst)
                .lines
                .is_empty()
        );
        assert!(
            Suppression::parse("//ttt: on\nvisible", DocumentKind::Tex)
                .lines
                .is_empty()
        );
    }

    #[test]
    fn typst_strings_raw_fences_and_block_comments_cannot_start_or_stop_regions() {
        for sample in [
            "#let example = \"//ttt: on\"",
            "#let example = \"example\n//ttt: on\nhidden?\"",
            "`//ttt: on`",
            "```typ\n//ttt: on\n```",
            "/*\n//ttt: on\n*/",
            "/* nested /* //ttt: on */ comments */",
            "\\/\\/ttt: on",
        ] {
            let source = format!("{sample}\nvisible");
            assert!(
                Suppression::parse(&source, DocumentKind::Typst)
                    .lines
                    .is_empty(),
                "{source}"
            );
        }
        let source = "//ttt: on\n```typ\n//ttt: off\n```\nstill hidden\n//ttt: off\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Typst), [2, 3, 4, 5]);
    }

    #[test]
    fn tex_escaped_percent_and_inline_literal_examples_are_not_directives() {
        for sample in [
            "\\%ttt: on",
            "\\verb|%ttt: on|",
            "\\lstinline[language=TeX]{\n%ttt: on\n}",
            "\\mintinline{tex}{\n%ttt: on\n}",
            "\\verb§%ttt: on§",
        ] {
            let source = format!("{sample}\nvisible");
            assert!(
                Suppression::parse(&source, DocumentKind::Tex)
                    .lines
                    .is_empty(),
                "{source}"
            );
        }
        // An escaped backslash does not consume the following percent, but the
        // comment is still not standalone and therefore is not a directive.
        assert!(
            Suppression::parse("\\\\%ttt: on\nvisible", DocumentKind::Tex)
                .lines
                .is_empty()
        );
        let source = "\\verb|unterminated\n%ttt: on\nhidden\n%ttt: off\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [3]);
    }

    #[test]
    fn tex_inline_literal_headers_keep_real_comments_but_exclude_payload_comments() {
        for command in [
            "\\lstinline",
            "\\lstinline[language=TeX]",
            "\\mintinline",
            "\\mintinline[breaklines]",
        ] {
            let payload = if command.starts_with("\\mintinline") {
                "{tex}{\n%ttt: off\n}"
            } else {
                "{\n%ttt: off\n}"
            };
            let source =
                format!("{command}\n%ttt: on\n{payload}\nstill hidden\n%ttt: off\nvisible");
            assert_eq!(
                suppressed(&source, DocumentKind::Tex),
                [3, 4, 5, 6],
                "{source}"
            );
        }
        let source = "%ttt: on\n\\mintinline\n%ttt: off\n{tex}{literal}\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [2]);
        let source = "\\mintinline{tex\n%ttt: on\n}{literal}\nstill hidden\n%ttt: off\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [3, 4]);
    }

    #[test]
    fn tex_verbatim_environments_and_their_unclosed_payloads_do_not_switch() {
        for environment in [
            "verbatim",
            "verbatim*",
            "Verbatim",
            "BVerbatim",
            "LVerbatim",
            "SaveVerbatim",
            "VerbatimOut",
            "lstlisting",
            "minted",
            "comment",
            "filecontents",
            "filecontents*",
        ] {
            for closing in [format!("\\end{{{environment}}}"), String::new()] {
                let source = format!("\\begin{{{environment}}}\n%ttt: on\n{closing}\nvisible");
                assert!(
                    Suppression::parse(&source, DocumentKind::Tex)
                        .lines
                        .is_empty(),
                    "{source}"
                );
            }
        }
        let source = "%ttt: on\n\\begin{verbatim}\n%ttt: off\n\\end{verbatim}\nstill hidden\n%ttt: off\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [2, 3, 4, 5]);
    }

    #[test]
    fn tex_comment_environment_requires_an_exact_standalone_closing_line() {
        for fake_end in [
            "Example: \\end{comment} closes this environment.",
            " \\end{comment}",
            "\\end{comment} trailing text",
            "\\end{comment}% trailing comment",
            "\\\\end{comment}",
        ] {
            for newline in ["\n", "\r\n"] {
                let source = [
                    "\\begin{comment}",
                    fake_end,
                    "%ttt: on",
                    "\\end{comment}",
                    "visible",
                    "%ttt: on",
                    "hidden",
                    "%ttt: off",
                    "visible again",
                ]
                .join(newline);
                assert_eq!(suppressed(&source, DocumentKind::Tex), [7], "{source}");
            }
        }
        let source = "\\begin{comment}\\end{comment}\n%ttt: on\n\\end{comment}\nvisible";
        assert!(
            Suppression::parse(source, DocumentKind::Tex)
                .lines
                .is_empty()
        );
        let source = "\\begin{comment}\nExample: \\end{comment}\n%ttt: on\nno real closing marker";
        assert!(
            Suppression::parse(source, DocumentKind::Tex)
                .lines
                .is_empty()
        );
    }

    #[test]
    fn tex_environment_headers_allow_spacing_and_real_comments() {
        let source = "\\begin\n%ttt: on\n{verbatim}\n%ttt: off\n\\end{verbatim}\nstill hidden\n%ttt: off\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [3, 4, 5, 6]);
        let source = "\\begin% header\n {verbatim}\n%ttt: on\n\\end{verbatim}\nvisible";
        assert!(
            Suppression::parse(source, DocumentKind::Tex)
                .lines
                .is_empty()
        );
    }

    #[test]
    fn unicode_content_and_comments_preserve_canonical_source_line_numbers() {
        let source =
            "中文😀\n// unrelated é\n//ttt: on\n中文 tyop\n// also hidden\n//ttt: off\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Typst), [4, 5]);
        assert!(
            Suppression::parse("ordinary text without directives", DocumentKind::Typst)
                .lines
                .is_empty()
        );
    }
}
