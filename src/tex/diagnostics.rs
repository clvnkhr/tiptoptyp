//! Contextual explanations for a verified Badness parse cascade. This never
//! creates a finding unless both original endpoint diagnostics are present.
use crate::{delimiters::TexDelimiterIndex, lsp::protocol::LspDiagnostic};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tiptoptyp_core::text::{LspPosition, LspRange};

const MESSAGE: &str = "Blank line inside display math. Remove it or make it a % comment.";
const CODE: &str = "math-paragraph-break";
type Position = (u32, u32);
type Bytes = (usize, usize);

/// Normalize before projection mapping, against the exact provider snapshot.
/// Preparation makes bounded linear passes; endpoint/blank-line queries are ordered
/// lookups, never a whole-document scan for every diagnostic.
pub(super) fn normalize_badness(source: &str, diagnostics: &mut Vec<LspDiagnostic>) {
    let opener = |diagnostic: &LspDiagnostic| {
        diagnostic.code.as_ref().and_then(Value::as_str) == Some("unclosed-math-delimiter")
    };
    let closer = |diagnostic: &LspDiagnostic| {
        (diagnostic.code.is_none()
            || diagnostic.code.as_ref().and_then(Value::as_str) == Some("parse"))
            && diagnostic.message.trim() == "unmatched `\\]`"
    };
    if !diagnostics.iter().any(opener) || !diagnostics.iter().any(closer) {
        return;
    }
    let mut positions = BTreeMap::new();
    for diagnostic in diagnostics.iter().filter(|d| opener(d) || closer(d)) {
        for position in [diagnostic.range.start, diagnostic.range.end] {
            positions.insert(key(position), None);
        }
    }
    resolve_positions(source, &mut positions);
    let bytes = |diagnostic: &LspDiagnostic| -> Option<Bytes> {
        Some((
            positions[&key(diagnostic.range.start)]?,
            positions[&key(diagnostic.range.end)]?,
        ))
    };
    let mut opens: BTreeMap<Bytes, Vec<usize>> = BTreeMap::new();
    let mut closes: BTreeMap<Bytes, Vec<usize>> = BTreeMap::new();
    for (index, diagnostic) in diagnostics.iter().enumerate() {
        if opener(diagnostic)
            && let Some(range) = bytes(diagnostic)
            && source.get(range.0..range.1) == Some("\\[")
        {
            opens.entry(range).or_default().push(index);
        } else if closer(diagnostic)
            && let Some(range) = bytes(diagnostic)
            && source.get(range.0..range.1) == Some("\\]")
        {
            closes.entry(range).or_default().push(index);
        }
    }
    if opens.is_empty() || closes.is_empty() {
        return;
    }
    let delimiters = TexDelimiterIndex::new(source);
    if has_conditionals(source, &delimiters) {
        return;
    }
    let mut blanks = Vec::new();
    let mut start = 0;
    for (line, text) in source.split_inclusive('\n').enumerate() {
        let body = text.strip_suffix('\n').unwrap_or(text);
        let body = body.strip_suffix('\r').unwrap_or(body);
        if body.bytes().all(|byte| byte.is_ascii_whitespace()) && !delimiters.ignored_at(start) {
            blanks.push((start, start + body.len(), line as u32, body.len() as u32));
        }
        start += text.len();
    }
    let mut removed = BTreeSet::new();
    let mut explained = BTreeSet::new();
    let mut replacements = BTreeMap::new();
    for ((start, end), open_indices) in opens {
        let Some([open, close]) = delimiters.pair_at(start) else {
            continue;
        };
        if open != (start..end) {
            continue;
        }
        let Some(close_indices) = closes.get(&(close.start, close.end)) else {
            continue;
        };
        let at = blanks.partition_point(|blank| blank.0 < open.end);
        let Some(&(blank_start, blank_end, line, column)) = blanks.get(at) else {
            continue;
        };
        if blank_end > close.start {
            continue;
        }
        removed.extend(open_indices.iter().copied());
        removed.extend(close_indices.iter().copied());
        if !explained.insert(blank_start) {
            continue;
        }
        let range = LspRange {
            start: LspPosition::new(line, 0),
            end: LspPosition::new(line, column),
        };
        let original = &diagnostics[open_indices[0]];
        let raw = json!({"range":range,"severity":1,"code":CODE,"source":original.source,"message":MESSAGE});
        let mut diagnostic = original.clone();
        diagnostic.range = range;
        diagnostic.code = Some(CODE.into());
        diagnostic.severity = Some(crate::lsp::protocol::DiagnosticSeverity::Error);
        diagnostic.message = MESSAGE.into();
        diagnostic.raw = raw;
        replacements.insert(open_indices[0], diagnostic);
    }
    if replacements.is_empty() {
        return;
    }
    *diagnostics = std::mem::take(diagnostics)
        .into_iter()
        .enumerate()
        .filter_map(|(index, diagnostic)| {
            replacements
                .remove(&index)
                .or_else(|| (!removed.contains(&index)).then_some(diagnostic))
        })
        .collect();
}

/// Without expansion we cannot tell whether a blank line reaches TeX at all.
/// Custom `\if...` control words are deliberately treated conservatively too.
fn has_conditionals(source: &str, delimiters: &TexDelimiterIndex) -> bool {
    let bytes = source.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'\\' {
            at += 1;
            continue;
        }
        let start = at;
        at += 1;
        let word = at;
        while bytes
            .get(at)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'@')
        {
            at += 1;
        }
        if at == word {
            // A control symbol consumes its next character, including `\\`.
            at += source[at..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        let command = &source[word..at];
        if (command.starts_with("if") || matches!(command, "else" | "or" | "fi" | "unless"))
            && !delimiters.ignored_at(start)
        {
            return true;
        }
    }
    false
}

fn key(position: LspPosition) -> Position {
    (position.line.get(), position.character.get())
}

fn resolve_positions(source: &str, positions: &mut BTreeMap<Position, Option<usize>>) {
    let mut chars = source.char_indices().peekable();
    let (mut line, mut column, mut byte) = (0u32, 0u32, 0usize);
    for (&(target_line, target_column), resolved) in positions.iter_mut() {
        while line < target_line || (line == target_line && column < target_column) {
            let Some(&(at, ch)) = chars.peek() else {
                break;
            };
            if line == target_line
                && (ch == '\n' || (ch == '\r' && source.as_bytes().get(at + 1) == Some(&b'\n')))
            {
                break;
            }
            chars.next();
            byte = at + ch.len_utf8();
            if ch == '\n' {
                line += 1;
                column = 0;
            } else {
                column += ch.len_utf16() as u32;
            }
        }
        if line == target_line && column == target_column {
            *resolved = Some(byte);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::protocol;
    use tiptoptyp_core::text::{ScalarOffset, lsp_position_at_scalar};

    fn diagnostic(source: &str, marker: &str, code: &str, message: &str) -> LspDiagnostic {
        let byte = source.find(marker).unwrap();
        let start = source[..byte].chars().count();
        let range = LspRange {
            start: lsp_position_at_scalar(source, ScalarOffset::new(start)),
            end: lsp_position_at_scalar(source, ScalarOffset::new(start + marker.chars().count())),
        };
        protocol::parse_diagnostic(
            &json!({"range":range,"severity":2,"code":code,"source":"badness","message":message}),
        )
        .unwrap()
    }
    fn cascade(source: &str) -> Vec<LspDiagnostic> {
        vec![
            diagnostic(
                source,
                "\\[",
                "unclosed-math-delimiter",
                "`\\[` has no matching `\\]` (unclosed display math)",
            ),
            diagnostic(source, "\\]", "parse", "unmatched `\\]`"),
        ]
    }
    #[test]
    fn explains_empty_and_ascii_whitespace_paragraphs_at_the_first_bad_line() {
        for blank in ["", " ", "\t  "] {
            let source = format!("α😀\r\n\\[\r\nx + y\r\n{blank}\r\n\r\n\\]\r\n");
            let mut diagnostics = cascade(&source);
            normalize_badness(&source, &mut diagnostics);
            assert_eq!(diagnostics.len(), 1);
            let diagnostic = &diagnostics[0];
            assert_eq!(diagnostic.code, Some(CODE.into()));
            assert_eq!(diagnostic.range.start, LspPosition::new(3, 0));
            assert_eq!(
                diagnostic.range.end,
                LspPosition::new(3, blank.len() as u32)
            );
            assert!(diagnostic.message.contains("% comment"));
            assert_eq!(diagnostic.raw["range"], json!(diagnostic.range));
            assert_eq!(diagnostic.raw["severity"], 1);
        }
    }
    #[test]
    fn valid_multiline_and_comment_lines_never_get_new_findings() {
        for middle in ["", "%\n", "  % math continuation\n"] {
            let source = format!("\\[\nx + y\n{middle}\\]\n");
            let mut diagnostics = cascade(&source);
            let before = diagnostics.clone();
            normalize_badness(&source, &mut diagnostics);
            assert_eq!(diagnostics, before);
            normalize_badness(&source, &mut Vec::new());
        }
    }
    #[test]
    fn escaped_and_verbatim_markers_are_never_reinterpreted_as_math() {
        for source in [
            "\\\\[\nx\n\n\\\\]\n",
            "\\begin{verbatim}\n\\[\nx\n\n\\]\n\\end{verbatim}\n",
        ] {
            let mut diagnostics = cascade(source);
            let before = diagnostics.clone();
            normalize_badness(source, &mut diagnostics);
            assert_eq!(diagnostics, before);
        }
    }
    #[test]
    fn conditionals_keep_the_original_cascade_instead_of_explaining_skipped_paragraphs() {
        for source in [
            "\\[\n\\iffalse\n\n\\fi x+y\n\\]",
            "\\iffalse ignored\\fi\n\\[\nx+y\n\n\\]",
        ] {
            let mut diagnostics = cascade(source);
            let before = diagnostics.clone();
            normalize_badness(source, &mut diagnostics);
            assert_eq!(diagnostics, before);
        }
        for command in [
            "if",
            "iftrue",
            "ifcustom",
            "if@custom",
            "fi",
            "else",
            "or",
            "unless",
        ] {
            let source = format!("\\{command}");
            assert!(has_conditionals(&source, &TexDelimiterIndex::new(&source)));
        }
    }
    #[test]
    fn conditional_guard_ignores_comments_literals_escapes_and_control_word_suffixes() {
        for prefix in [
            "% \\iffalse\n",
            "\\begin{verbatim}\n\\iffalse\n\\end{verbatim}\n",
            "\\verb|\\iffalse|\n",
            "\\\\iffalse\n",
            "\\\\\\\\iftrue\n",
            "\\field \\elsewhere \\orange \\unlessness\n",
        ] {
            let source = format!("{prefix}\\[\nx+y\n\n\\]");
            let mut diagnostics = cascade(&source);
            normalize_badness(&source, &mut diagnostics);
            assert_eq!(diagnostics.len(), 1, "{prefix}");
            assert_eq!(diagnostics[0].code, Some(CODE.into()), "{prefix}");
        }
        let source = "\\\\\\iffalse";
        assert!(has_conditionals(source, &TexDelimiterIndex::new(source)));
    }
    #[test]
    fn unrelated_parse_findings_and_incomplete_cascades_remain_exact() {
        let source = "\\[\nx\n\n\\]\n{oops";
        let unrelated = diagnostic(source, "{oops", "parse", "unclosed group");
        let mut diagnostics = cascade(source);
        diagnostics.push(unrelated.clone());
        normalize_badness(source, &mut diagnostics);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[1], unrelated);
        let mut incomplete = vec![cascade(source).remove(0)];
        let before = incomplete.clone();
        normalize_badness(source, &mut incomplete);
        assert_eq!(incomplete, before);
        let source = "\\[\nx\n\n";
        let mut unclosed = vec![diagnostic(
            source,
            "\\[",
            "unclosed-math-delimiter",
            "unclosed display math",
        )];
        let before = unclosed.clone();
        normalize_badness(source, &mut unclosed);
        assert_eq!(unclosed, before);
    }
    #[test]
    fn real_badness_lsp_uncoded_parse_error_and_close_first_order_are_supported() {
        let source = "\\[\nx+y\n\n\\]\n";
        let mut diagnostics = cascade(source);
        diagnostics[1].code = None;
        diagnostics[1].raw.as_object_mut().unwrap().remove("code");
        diagnostics.swap(0, 1);
        normalize_badness(source, &mut diagnostics);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, Some(CODE.into()));
        assert_eq!(diagnostics[0].range.start, LspPosition::new(2, 0));
    }

    #[test]
    #[ignore = "bounded optimized baseline/after diagnostic-event component measurement"]
    fn paragraph_normalization_cost_probe() {
        let source = format!("{}\\[\nx+y\n\n\\]\n", "Ordinary α prose.\n".repeat(5_000));
        let unrelated = diagnostic(&source, "Ordinary", "other-rule", "An unrelated finding");
        let mut many = vec![unrelated; 1_000];
        let mut cascade = cascade(&source);
        cascade.append(&mut many.clone());
        for (case, findings) in [
            ("clean", Vec::new()),
            ("unrelated", std::mem::take(&mut many)),
            ("cascade", cascade),
        ] {
            for enabled in [false, true] {
                for _ in 0..10 {
                    let mut diagnostics = findings.clone();
                    if enabled {
                        normalize_badness(&source, &mut diagnostics);
                    }
                    drop(std::hint::black_box(diagnostics));
                }
                for batch in 0..5 {
                    let started = std::time::Instant::now();
                    for _ in 0..10 {
                        let mut diagnostics = findings.clone();
                        if enabled {
                            normalize_badness(&source, &mut diagnostics);
                        }
                        drop(std::hint::black_box(diagnostics));
                    }
                    println!(
                        "paragraph_normalization_cost,case={case},enabled={enabled},source_bytes={},findings={},iterations=10,batch={batch},ns_per_event={}",
                        source.len(),
                        findings.len(),
                        started.elapsed().as_nanos() / 10
                    );
                }
            }
        }
    }

    #[test]
    fn duplicate_cascades_coalesce_but_nonmatching_close_messages_stay_raw() {
        let source = "\\[\nx\n\n\\]";
        let mut diagnostics = cascade(source);
        diagnostics.extend(cascade(source));
        normalize_badness(source, &mut diagnostics);
        assert_eq!(diagnostics.len(), 1);
        let mut diagnostics = cascade(source);
        diagnostics[1].message = "a different parse problem".into();
        let before = diagnostics.clone();
        normalize_badness(source, &mut diagnostics);
        assert_eq!(diagnostics, before);
    }
}
