//! Offline writing checks. Locations are Unicode scalar offsets, like Harper.
use crate::{
    diagnostics::{Diagnostic, DiagnosticLocation, DiagnosticSeverity, DiagnosticSource},
    document::DocumentKind,
};
use harper_core::{
    Dialect, Document,
    linting::{LintGroup, Linter},
    spell::{FstDictionary, MergedDictionary},
};
use std::{
    path::Path,
    sync::{Arc, LazyLock},
};

const MATH_WORDS: &str = include_str!("../assets/dictionaries/mathematics.txt");

// Construct once on the writing worker, and reuse for parsing and linting. Keep
// Harper's richer metadata for existing words; the supplement adds spellings
// without guessing their grammatical roles or accepting arbitrary affixes.
static WRITING_DICTIONARY: LazyLock<Arc<MergedDictionary>> = LazyLock::new(|| {
    use harper_core::spell::Dictionary;
    let curated = FstDictionary::curated();
    let words = MATH_WORDS
        .lines()
        .filter(|word| !curated.contains_word_str(word))
        .map(|word| {
            (
                word.chars().collect(),
                harper_core::DictWordMetadata::default(),
            )
        })
        .collect();
    let mut dictionary = MergedDictionary::new();
    dictionary.add_dictionary(curated);
    dictionary.add_dictionary(Arc::new(FstDictionary::new(words)));
    Arc::new(dictionary)
});

pub(crate) fn check(
    source: &str,
    kind: DocumentKind,
    path: &Path,
    grammar: bool,
    unicode: bool,
    language: crate::settings::WritingLanguage,
) -> Vec<Diagnostic> {
    let mut issues: Vec<(usize, String, &'static str)> = Vec::new();
    if grammar
        && matches!(kind, DocumentKind::Typst | DocumentKind::Tex)
        && let Some(dialect) = writing_dialect(source, kind, language)
    {
        let dictionary = Arc::clone(&WRITING_DICTIONARY);
        let document = if kind == DocumentKind::Tex {
            Document::new(source, &harper_tex::TeX::default(), dictionary.as_ref())
        } else {
            Document::new(source, &harper_typst::Typst, dictionary.as_ref())
        };
        let mut linter = LintGroup::new_curated(dictionary, dialect);
        let excluded = excluded_typst_ranges(source, kind);
        let characters: Vec<char> = source.chars().collect();
        let indentation = indentation_ranges(&characters);
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
                    let next = indentation.partition_point(|range| range.end <= lint.span.start);
                    !indentation.get(next).is_some_and(|range| {
                        range.start <= lint.span.start && lint.span.end <= range.end
                    })
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
/// Preserve Harper's original scalar coordinates; indentation is source layout,
/// not prose. Inline whitespace and all non-whitespace lints remain eligible.
fn indentation_ranges(characters: &[char]) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let mut line_start = true;
    for (index, &ch) in characters.iter().enumerate() {
        if line_start && matches!(ch, ' ' | '\t') {
            if let Some(last) = ranges.last_mut().filter(|last| last.end == index) {
                last.end += 1;
            } else {
                ranges.push(index..index + 1);
            }
        } else {
            line_start = matches!(ch, '\r' | '\n');
        }
    }
    ranges
}

/// Reads syntax rather than matching comments, examples, nested functions or strings.
fn writing_dialect(
    source: &str,
    kind: DocumentKind,
    language: crate::settings::WritingLanguage,
) -> Option<Dialect> {
    use crate::settings::WritingLanguage;
    use typst_syntax::ast::{Arg, Expr, SetRule};
    match language {
        WritingLanguage::British => return Some(Dialect::British),
        WritingLanguage::American => return Some(Dialect::American),
        WritingLanguage::Auto => {}
    }
    let mut lang = None;
    let mut region = None;
    if kind == DocumentKind::Typst {
        let syntax = typst_syntax::parse(source);
        for node in syntax.children() {
            let Some(rule) = node.cast::<SetRule>() else {
                continue;
            };
            if !matches!(rule.target(), Expr::Ident(name) if name.as_str() == "text")
                || rule.condition().is_some()
            {
                continue;
            }
            for arg in rule.args().items() {
                if let Arg::Named(named) = arg
                    && let Expr::Str(value) = named.expr()
                {
                    match named.name().as_str() {
                        "lang" => lang = Some(value.get().to_ascii_lowercase()),
                        "region" => region = Some(value.get().to_ascii_lowercase()),
                        _ => {}
                    }
                }
            }
        }
    }
    match lang.as_deref() {
        Some("en-us") => Some(Dialect::American),
        Some("en-gb" | "en-uk") => Some(Dialect::British),
        Some("en") | None => Some(if region.as_deref() == Some("us") {
            Dialect::American
        } else {
            Dialect::British
        }),
        _ => None, // Harper checks English; `uk` is Ukrainian, not British English.
    }
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
    fn mathematical_prose_is_accepted_in_typst_and_tex_without_hiding_errors() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let source = "These semigroups are nonautonomous.\nThe eigenfunctions are quasilinear.\nThis is the the misspelld conclusion.";
            let issues = check(
                source,
                kind,
                Path::new("fixture"),
                true,
                false,
                crate::settings::WritingLanguage::British,
            );
            assert!(
                issues.iter().all(|issue| issue.location.unwrap().line == 3),
                "{kind:?}: {issues:?}"
            );
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.location.unwrap().column == 9),
                "repeated words must still be checked: {issues:?}"
            );
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.location.unwrap().column == 17),
                "ordinary misspellings must still be checked: {issues:?}"
            );
        }
    }

    #[test]
    fn mathematical_typos_and_repeated_terms_are_reported_in_both_adapters() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let issues = check(
                "This semigrop is nonautonomus.\nThis operator is quasiliner.\nThis is a semigroup semigroup.",
                kind,
                Path::new("fixture"),
                true,
                false,
                crate::settings::WritingLanguage::American,
            );
            for (line, column) in [(1, 6), (1, 18), (2, 18)] {
                assert!(
                    issues
                        .iter()
                        .any(|issue| issue.location == Some(DiagnosticLocation { line, column })),
                    "{kind:?}: typo at {line}:{column} must be reported: {issues:?}"
                );
            }
            assert!(
                issues.iter().any(|issue| issue.location.unwrap().line == 3),
                "repeated mathematical words must be reported: {issues:?}"
            );
        }
    }

    #[test]
    fn math_dictionary_is_reviewed_and_reused_and_suggests_math_spellings() {
        use harper_core::spell::Dictionary;
        let words: Vec<_> = MATH_WORDS.lines().collect();
        assert!(words.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            words
                .iter()
                .all(|word| word.len() >= 4
                    && word.bytes().all(|letter| letter.is_ascii_lowercase()))
        );
        let dictionary = Arc::clone(&WRITING_DICTIONARY);
        assert!(Arc::ptr_eq(&dictionary, &WRITING_DICTIONARY));
        for word in ["color", "colour", "running", "children", "British"] {
            assert_eq!(
                dictionary.get_word_metadata_str(word),
                FstDictionary::curated().get_word_metadata_str(word),
                "existing grammatical and dialect metadata must survive: {word}"
            );
        }
        for word in words {
            assert!(dictionary.contains_word_str(word), "{word}");
        }
        for typo in [
            "semigrop",
            "nonautonomus",
            "quasiliner",
            "nonautonomouss",
            "propogator",
            "hler",
        ] {
            assert!(!dictionary.contains_word_str(typo), "{typo}");
        }
        assert!(
            dictionary
                .fuzzy_match_str("semigrop", 1, 10)
                .iter()
                .any(|result| { result.word.iter().collect::<String>() == "semigroup" })
        );
    }

    #[test]
    #[ignore = "opt-in optimized dictionary parse/lint cost comparison"]
    fn math_dictionary_cost_measurement() {
        use harper_core::spell::Dictionary;
        if cfg!(debug_assertions) {
            panic!("run with --release");
        }
        fn measure(dictionary: Arc<impl Dictionary + 'static>, source: &str) -> u128 {
            let mut samples = Vec::new();
            for iteration in 0..8 {
                let started = std::time::Instant::now();
                let document = Document::new(source, &harper_typst::Typst, dictionary.as_ref());
                let mut linter = LintGroup::new_curated(Arc::clone(&dictionary), Dialect::British);
                std::hint::black_box(linter.lint(&document));
                if iteration >= 3 {
                    samples.push(started.elapsed().as_micros());
                }
            }
            samples.sort_unstable();
            samples[samples.len() / 2]
        }
        let source = "This is the the example.\n$ x^2 + y^2 = z^2 $\n".repeat(40);
        let baseline = measure(FstDictionary::curated(), &source);
        let supplemented = measure(Arc::clone(&WRITING_DICTIONARY), &source);
        eprintln!(
            "math-dictionary-cost arch={} profile=release bytes={} warmup=3 samples=5 baseline_us={baseline} supplemented_us={supplemented}",
            std::env::consts::ARCH,
            source.len()
        );
    }

    #[test]
    fn leading_indentation_is_not_prose_spacing_but_inline_spaces_and_typos_are() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let source = "This is a sentence\r\n    with a misspelld word.\r\n\t\tThis is  another sentence.";
            let issues = check(
                source,
                kind,
                Path::new("fixture"),
                true,
                false,
                crate::settings::WritingLanguage::British,
            );
            assert!(
                !issues
                    .iter()
                    .any(|d| d.message.contains("spaces") && d.location.unwrap().column <= 4),
                "{kind:?}: {issues:?}"
            );
            assert!(
                issues
                    .iter()
                    .any(|d| d.message.contains("spaces") && d.location.unwrap().line == 3),
                "{issues:?}"
            );
            assert!(
                issues
                    .iter()
                    .any(|d| d.location.is_some_and(|l| l.line == 2 && l.column == 12)),
                "typo coordinates must retain indentation: {issues:?}"
            );
        }
    }
    #[test]
    fn leading_indentation_is_ignored_including_multiline_strings() {
        for (kind, source) in [
            (DocumentKind::Typst, "    Hello world."),
            (DocumentKind::Tex, "\t\tHello world."),
            (DocumentKind::Typst, "#text(\"Hello\n    world.\")"),
        ] {
            let issues = check(
                source,
                kind,
                Path::new("fixture"),
                true,
                false,
                crate::settings::WritingLanguage::British,
            );
            assert!(issues.is_empty(), "{source:?}: {issues:?}");
        }
    }
    #[test]
    fn language_detection_uses_real_top_level_text_rules_and_explicit_overrides() {
        use crate::settings::WritingLanguage::{American, Auto, British};
        for (source, expected) in [
            (
                "#set text(lang: \"en\", region: \"us\")",
                Some(Dialect::American),
            ),
            (
                "#set text(lang: \"en\", region: \"gb\")",
                Some(Dialect::British),
            ),
            ("#set text(lang: \"uk\")", None),
            (
                "// #set text(lang: \"en\", region: \"us\")",
                Some(Dialect::British),
            ),
            (
                "#let demo() = { set text(region: \"us\") }",
                Some(Dialect::British),
            ),
            ("#set text(region: \"us\") if false", Some(Dialect::British)),
        ] {
            assert_eq!(
                writing_dialect(source, DocumentKind::Typst, Auto),
                expected,
                "{source}"
            );
            assert_eq!(
                writing_dialect(source, DocumentKind::Typst, British),
                Some(Dialect::British)
            );
            assert_eq!(
                writing_dialect(source, DocumentKind::Typst, American),
                Some(Dialect::American)
            );
        }
    }

    #[test]
    fn technical_arguments_display_math_and_names_are_not_spelling_errors() {
        let source = "#set text(lang: \"en\", region: \"gb\", font: \"NonsensicalFontName\")\nThe Leray theorem has the the consequence.\n$ nonwordxyz + nonwordxyz $\nThiss is ordinary prose.";
        let issues = check(
            source,
            DocumentKind::Typst,
            Path::new("fixture.typ"),
            true,
            false,
            crate::settings::WritingLanguage::Auto,
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
                false,
                crate::settings::WritingLanguage::Auto,
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
                crate::settings::WritingLanguage::Auto,
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
            crate::settings::WritingLanguage::Auto,
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
                true,
                crate::settings::WritingLanguage::Auto,
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
            let issues = check(
                source,
                kind,
                Path::new("fixture"),
                true,
                false,
                crate::settings::WritingLanguage::Auto,
            );
            assert!(issues.iter().any(|issue| issue.location.unwrap().line == 1));
            assert!(
                !issues
                    .iter()
                    .any(|issue| issue.message.contains("nonwordxyz"))
            );
        }
    }
}
