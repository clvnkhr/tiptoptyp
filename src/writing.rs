//! Offline writing checks. Locations are Unicode scalar offsets, like Harper.
use crate::{
    diagnostics::{Diagnostic, DiagnosticLocation, DiagnosticSeverity, DiagnosticSource},
    document::DocumentKind,
};
use harper_core::{
    Dialect, Document,
    linting::{LintGroup, Linter},
    spell::FstDictionary,
};
use std::path::Path;

pub(crate) fn check(
    source: &str,
    kind: DocumentKind,
    path: &Path,
    grammar: bool,
    unicode: bool,
) -> Vec<Diagnostic> {
    let mut issues: Vec<(usize, String, &'static str)> = Vec::new();
    if grammar && matches!(kind, DocumentKind::Typst | DocumentKind::Tex) {
        let document = if kind == DocumentKind::Tex {
            Document::new_curated(source, &harper_tex::TeX::default())
        } else {
            Document::new_curated(source, &harper_typst::Typst)
        };
        let mut linter = LintGroup::new_curated(FstDictionary::curated(), Dialect::British);
        let excluded = excluded_typst_ranges(source, kind);
        let characters: Vec<char> = source.chars().collect();
        issues.extend(
            linter
                .lint(&document)
                .into_iter()
                .filter(|lint| {
                    let next = excluded.partition_point(|range| range.end <= lint.span.start);
                    excluded
                        .get(next)
                        .is_none_or(|range| range.start >= lint.span.end)
                })
                .filter(|lint| {
                    lint.lint_kind != harper_core::linting::LintKind::Spelling
                        || !likely_name(&characters, lint.span.start, lint.span.end)
                })
                .map(|lint| (lint.span.start, lint.message, "Harper")),
        );
    }
    if unicode {
        for (index, ch) in source.chars().enumerate() {
            let description = match ch {
                '\u{200b}' => Some("zero-width space"),
                '\u{200c}' => Some("zero-width non-joiner"),
                '\u{200d}' => Some("zero-width joiner"),
                '\u{2060}' => Some("word joiner"),
                '\u{feff}' => Some("embedded byte-order mark"),
                '\u{00ad}' => Some("soft hyphen"),
                '\u{00a0}' => Some("non-breaking space"),
                '\u{202f}' => Some("narrow non-breaking space"),
                '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {
                    Some("bidirectional control")
                }
                '\u{3000}' => Some("ideographic space"),
                '\u{ff01}'..='\u{ff5e}' => Some("full-width ASCII lookalike"),
                _ => None,
            };
            if let Some(description) = description {
                issues.push((
                    index,
                    format!("U+{:04X}: {description}", ch as u32),
                    "Unicode",
                ));
            }
        }
        // Only flag visually Latin letters inside an otherwise ASCII word;
        // ordinary Greek, Cyrillic, and CJK prose is not suspicious by itself.
        let mut start = 0;
        for word in source.split_inclusive(|c: char| !c.is_alphanumeric() && c != '_') {
            if word.chars().any(|c| c.is_ascii_alphabetic()) {
                for (offset, ch) in word.chars().enumerate() {
                    if "аесорхуАВСЕНКМОРТХіјѕΙΟΡΧορχ".contains(ch) {
                        issues.push((
                            start + offset,
                            format!(
                                "U+{:04X}: a Latin lookalike in a mixed-script word",
                                ch as u32
                            ),
                            "Unicode",
                        ));
                    }
                }
            }
            start += word.chars().count();
        }
    }
    issues.sort_by_key(|issue| issue.0);
    let mut locations = source
        .chars()
        .scan((1, 1), |position, ch| {
            let current = *position;
            if ch == '\n' {
                position.0 += 1;
                position.1 = 1;
            } else {
                position.1 += 1;
            }
            Some(current)
        })
        .collect::<Vec<_>>();
    locations.push((source.lines().count().max(1), 1));
    issues
        .into_iter()
        .take(1000)
        .map(|(offset, message, provider)| {
            let (line, column) = locations.get(offset).copied().unwrap_or((1, 1));
            Diagnostic {
                provider: Some(provider.into()),
                severity: DiagnosticSeverity::Help,
                source: DiagnosticSource::File(path.to_owned()),
                location: Some(DiagnosticLocation { line, column }),
                message,
                details: vec![],
            }
        })
        .collect()
}
// Harper deliberately reads Typst string literals as prose. Technical named
// arguments and math are not prose; keep their original scalar coordinates.
fn excluded_typst_ranges(source: &str, kind: DocumentKind) -> Vec<std::ops::Range<usize>> {
    if kind != DocumentKind::Typst {
        return Vec::new();
    }
    use typst_syntax::{LinkedNode, SyntaxKind, ast::Named};
    fn visit(node: LinkedNode<'_>, offsets: &[usize], out: &mut Vec<std::ops::Range<usize>>) {
        let excluded = node.kind() == SyntaxKind::Equation
            || node.get().cast::<Named>().is_some_and(|arg| {
                !matches!(
                    arg.name().as_str(),
                    "body" | "title" | "caption" | "alt" | "description"
                )
            });
        if excluded {
            let range = node.range();
            out.push(offsets[range.start]..offsets[range.end]);
        } else {
            for child in node.children() {
                visit(child, offsets, out);
            }
        }
    }
    let mut offsets = vec![0; source.len() + 1];
    for (scalar, (byte, ch)) in source.char_indices().enumerate() {
        offsets[byte..byte + ch.len_utf8()].fill(scalar);
        offsets[byte + ch.len_utf8()] = scalar + 1;
    }
    let syntax = typst_syntax::Source::detached(source);
    let mut excluded = Vec::new();
    visit(LinkedNode::new(syntax.root()), &offsets, &mut excluded);
    excluded
}

// A capitalized word in the middle of a sentence is usually a person's or
// place's name. Do not exempt sentence-initial spelling mistakes or lowercase
// prose, and retain every non-spelling rule for names.
fn likely_name(source: &[char], start: usize, end: usize) -> bool {
    let Some(word) = source.get(start..end) else {
        return false;
    };
    if word.len() < 2 || !word[0].is_uppercase() || !word[1..].iter().all(|ch| ch.is_lowercase()) {
        return false;
    }
    source[..start]
        .iter()
        .rev()
        .find(|ch| **ch == '\n' || !ch.is_whitespace())
        .is_some_and(|ch| !matches!(ch, '.' | '!' | '?' | ':' | '\n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn technical_arguments_display_math_and_names_are_not_spelling_errors() {
        let source = "#set text(lang: \"uk\", font: \"NonsensicalFontName\")\nThe Leray theorem has the the consequence.\n$ nonwordxyz + nonwordxyz $\nThiss is ordinary prose.";
        let issues = check(
            source,
            DocumentKind::Typst,
            Path::new("fixture.typ"),
            true,
            false,
        );
        assert!(
            !issues
                .iter()
                .any(|d| d.location.unwrap().line == 1 || d.location.unwrap().line == 3)
        );
        assert!(!issues.iter().any(|d| d.message.contains("Leray")));
        assert!(
            issues.iter().any(|d| d.location.unwrap().line == 4),
            "{issues:?}"
        );
        assert!(issues.iter().any(|d| d.location.unwrap().line == 2));
        let tex = r"The Leray theorem. \[ nonwordxyz \] \begin{equation} nonwordxyz \end{equation}";
        assert!(
            !check(
                tex,
                DocumentKind::Tex,
                Path::new("fixture.tex"),
                true,
                false
            )
            .iter()
            .any(|d| d.message.contains("nonwordxyz") || d.message.contains("Leray"))
        );
    }
    #[test]
    #[ignore = "opt-in optimized writing-check cost measurement"]
    fn writing_cost_measurement() {
        if cfg!(debug_assertions) {
            panic!("run with --release");
        }
        let source = "This is the the example.\n$ x^2 + y^2 = z^2 $\n".repeat(40);
        for (name, grammar, unicode) in [
            ("disabled", false, false),
            ("cold-grammar", true, false),
            ("warm-grammar", true, false),
            ("unicode", false, true),
        ] {
            let started = std::time::Instant::now();
            let results = check(
                &source,
                DocumentKind::Typst,
                Path::new("fixture.typ"),
                grammar,
                unicode,
            );
            eprintln!(
                "writing-cost {name} arch={} bytes={} elapsed_us={} diagnostics={}",
                std::env::consts::ARCH,
                source.len(),
                started.elapsed().as_micros(),
                results.len()
            );
        }
    }
    #[test]
    fn suspicious_unicode_locations_respect_multibyte_prose() {
        let issues = check(
            "正常中文\nаpple\u{200b}！",
            DocumentKind::Typst,
            Path::new("test.typ"),
            false,
            true,
        );
        assert_eq!(issues.len(), 3);
        assert_eq!(
            issues[0].location,
            Some(DiagnosticLocation { line: 2, column: 1 })
        );
        assert_eq!(issues[1].location.unwrap().column, 6);
        assert!(
            check(
                "普通中文 Ελληνικά русский",
                DocumentKind::Typst,
                Path::new("t.typ"),
                false,
                true
            )
            .is_empty()
        );
    }
    #[test]
    fn grammar_checks_prose_but_skips_typesetting_math() {
        for (kind, source) in [
            (
                DocumentKind::Typst,
                "This is the the example.\n$ nonwordxyz + nonwordxyz $",
            ),
            (
                DocumentKind::Tex,
                "This is the the example.\n$ nonwordxyz + nonwordxyz $",
            ),
        ] {
            let issues = check(source, kind, Path::new("fixture"), true, false);
            assert!(issues.iter().any(|issue| issue.location.unwrap().line == 1));
            assert!(
                !issues
                    .iter()
                    .any(|issue| issue.message.contains("nonwordxyz"))
            );
        }
    }
}
