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
        issues.extend(
            linter
                .lint(&document)
                .into_iter()
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
#[cfg(test)]
mod tests {
    use super::*;
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
