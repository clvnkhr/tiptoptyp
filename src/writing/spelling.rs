//! Explicit mathematical spellings and compounds, without accepting arbitrary affixes.

use harper_core::{Token, TokenKind, spell::Dictionary};
use std::{borrow::Cow, sync::LazyLock};

// These are dictionary entries, not a normalization of the document. The
// original Unicode character and scalar span remain available to every rule.
pub(super) fn dictionary_variants(word: &str) -> impl Iterator<Item = Cow<'_, str>> {
    let compound = word.contains('-');
    [
        Some(Cow::Borrowed(word)),
        compound.then(|| Cow::Owned(word.replace('-', "\u{2010}"))),
        compound.then(|| Cow::Owned(word.replace('-', "\u{2011}"))),
    ]
    .into_iter()
    .flatten()
}

static MAX_REVIEWED_COMPOUND_SCALARS: LazyLock<usize> = LazyLock::new(|| {
    super::MATH_WORDS
        .lines()
        .filter(|word| word.contains('-'))
        .map(str::len)
        .max()
        .unwrap_or(0)
});

/// Harper splits hyphenated words before dictionary lookup. Preserve a reviewed
/// whole mathematical compound as one word, so `well-posedness` does not create
/// an unknown standalone `posedness`. Unknown compounds retain their original
/// tokens and spellchecking; recognizing a prefix or suffix is never sufficient.
///
/// This compacts in place in one pass, retaining the token vector's capacity.
/// Each hyphen chain is visited once, and dictionary lookups are bounded by the
/// longest reviewed compound. Code, markup, whitespace and source gaps cannot
/// become a compound merely because their neighbouring words would match.
pub(super) fn merge_reviewed_compounds(source: &[char], tokens: &mut Vec<Token>) {
    let mut read = 0;
    let mut write = 0;
    while read < tokens.len() {
        let mut end = read + 1;
        if tokens[read].kind.is_word() {
            while end + 1 < tokens.len()
                && tokens[end + 1].kind.is_word()
                && compound_separator(source, &tokens[end])
                && tokens[end - 1].span.end == tokens[end].span.start
                && tokens[end].span.end == tokens[end + 1].span.start
            {
                end += 2;
            }
        }
        let span_start = tokens[read].span.start;
        let span_end = tokens[end - 1].span.end;
        let known = end > read + 1
            && span_end - span_start <= *MAX_REVIEWED_COMPOUND_SCALARS
            && super::WRITING_DICTIONARY.contains_word(&source[span_start..span_end]);
        if known {
            tokens[write] = Token::new(
                harper_core::Span::new(span_start, span_end),
                TokenKind::Word(None),
            );
            write += 1;
        } else {
            for index in read..end {
                if write != index {
                    tokens[write] = tokens[index].clone();
                }
                write += 1;
            }
        }
        read = end;
    }
    tokens.truncate(write);
}

fn compound_separator(source: &[char], token: &Token) -> bool {
    if token.span.len() != 1 {
        return false;
    }
    match source[token.span.start] {
        '-' => token.kind.is_hyphen(),
        // Harper 2.11 lexes these two hyphens as Unlintable characters.
        '\u{2010}' | '\u{2011}' => token.kind.is_unlintable(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        diagnostics::DiagnosticLocation, document::DocumentKind, settings::WritingLanguage,
    };
    use harper_core::{
        TokenKind,
        parsers::{Parser, PlainEnglish},
        spell::Dictionary,
    };
    use std::path::Path;

    #[test]
    fn known_compounds_are_accepted_without_accepting_compound_typos() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let source = "The theorem proves well-posedness and ill-posedness.\nThe theorem proves well-posedniss and ill-posedniss.\nThis is a well-posedness well-posedness result.";
            let issues = super::super::check(
                source,
                kind,
                Path::new("fixture"),
                true,
                false,
                WritingLanguage::British,
            );
            assert!(
                issues.iter().all(|issue| issue.location.unwrap().line != 1),
                "known mathematical compounds must not be reported: {kind:?}: {issues:?}"
            );
            for column in [25, 43] {
                assert!(
                    issues.iter().any(|issue| {
                        issue.location == Some(DiagnosticLocation { line: 2, column })
                    }),
                    "compound misspellings must still be reported: {kind:?}: {issues:?}"
                );
            }
            assert!(
                issues.iter().any(|issue| {
                    issue.location.unwrap().line == 3 && issue.message.contains("repeat")
                }),
                "repeated known compounds must still be reported: {kind:?}: {issues:?}"
            );
        }
    }
    #[test]
    fn unicode_hyphens_and_capitalized_compounds_keep_exact_source_spans() {
        for spelling in [
            "well-posedness",
            "well\u{2010}posedness",
            "well\u{2011}posedness",
            "Well-posedness",
        ] {
            let source: Vec<char> = spelling.chars().collect();
            let mut tokens = PlainEnglish.parse(&source);
            super::merge_reviewed_compounds(&source, &mut tokens);
            assert_eq!(tokens.len(), 1, "{spelling}");
            assert!(tokens[0].kind.is_word(), "{spelling}");
            assert_eq!(tokens[0].span.start, 0);
            assert_eq!(tokens[0].span.end, source.len());
            assert_eq!(tokens[0].get_str(&source), spelling);
            assert!(super::super::WRITING_DICTIONARY.contains_word(&source));
        }
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            for hyphen in ['-', '\u{2010}', '\u{2011}'] {
                let source =
                    format!("The theorem proves well{hyphen}posedness and ill{hyphen}posedness.");
                assert!(
                    super::super::check(
                        &source,
                        kind,
                        Path::new("fixture"),
                        true,
                        false,
                        WritingLanguage::British,
                    )
                    .is_empty(),
                    "{kind:?}: {source}"
                );
            }
        }
    }

    #[test]
    fn compound_recognition_never_crosses_spaces_source_gaps_or_masked_code() {
        for text in [
            "well -posedness",
            "well- posedness",
            "well - posedness",
            "well--posedness",
            "well–posedness",
            "well—posedness",
            "well-posedniss",
            "un-posedness",
            "well-posednessful",
            "well-posedness-typoo",
        ] {
            let source: Vec<char> = text.chars().collect();
            let mut tokens = PlainEnglish.parse(&source);
            let original = tokens.clone();
            super::merge_reviewed_compounds(&source, &mut tokens);
            assert_eq!(tokens, original, "{text}");
        }
        let source: Vec<char> = "well-posedness".chars().collect();
        let mut tokens = PlainEnglish.parse(&source);
        tokens.remove(1); // An adapter excluded the source's hyphen.
        let original = tokens.clone();
        super::merge_reviewed_compounds(&source, &mut tokens);
        assert_eq!(tokens, original);
        let mut tokens = PlainEnglish.parse(&source);
        tokens[1].kind = TokenKind::Unlintable; // The hyphen belongs to code.
        let original = tokens.clone();
        super::merge_reviewed_compounds(&source, &mut tokens);
        assert_eq!(tokens, original);
    }

    #[test]
    fn compound_compaction_reuses_capacity_and_long_unknown_chains_stay_unchanged() {
        for text in [
            "well-posedness ill-posedness quasi-linearity".to_owned(),
            format!("{}word", "misspelld-".repeat(10_000)),
        ] {
            let source: Vec<char> = text.chars().collect();
            let mut tokens = PlainEnglish.parse(&source);
            let allocation = tokens.as_ptr();
            let capacity = tokens.capacity();
            let original = tokens.clone();
            super::merge_reviewed_compounds(&source, &mut tokens);
            assert_eq!(tokens.as_ptr(), allocation);
            assert_eq!(tokens.capacity(), capacity);
            if text.starts_with("well") {
                assert!(tokens.len() < original.len());
            } else {
                assert_eq!(tokens, original);
            }
        }
    }

    #[test]
    fn operator_vocabulary_does_not_hide_near_misspellings_or_repeated_terms() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let source = "The accretive operator has accretivity, dissipativity and sectoriality.\nThe paradifferential calculus uses paraproducts and quasinormed spaces.\nThis implies hypocoercivity and hypercontractivity in the subcritical case.\nThis accretivty and hypercontractivty statement is misspelld.\nThis is a paraproduct paraproduct.";
            let issues = super::super::check(
                source,
                kind,
                Path::new("fixture"),
                true,
                false,
                WritingLanguage::American,
            );
            assert!(
                issues.iter().all(|issue| issue.location.unwrap().line >= 4),
                "{kind:?}: {issues:?}"
            );
            for column in [6, 21, 52] {
                assert!(
                    issues.iter().any(|issue| {
                        issue.location == Some(DiagnosticLocation { line: 4, column })
                    }),
                    "near misspellings must remain visible: {kind:?}: {issues:?}"
                );
            }
            assert!(
                issues.iter().any(|issue| {
                    issue.location.unwrap().line == 5 && issue.message.contains("repeat")
                }),
                "repeated terms must remain visible: {kind:?}: {issues:?}"
            );
        }
        assert!(!super::super::WRITING_DICTIONARY.contains_word_str("posedness"));
        assert!(!super::super::WRITING_DICTIONARY.contains_word_str("accretivty"));
        assert!(!super::super::WRITING_DICTIONARY.contains_word_str("hypercontractivty"));
    }

    #[test]
    fn dictionary_variants_are_explicit_and_sentence_dashes_stay_punctuation() {
        assert_eq!(
            super::dictionary_variants("semigroup").collect::<Vec<_>>(),
            ["semigroup"]
        );
        assert_eq!(
            super::dictionary_variants("well-posedness").collect::<Vec<_>>(),
            [
                "well-posedness",
                "well\u{2010}posedness",
                "well\u{2011}posedness"
            ]
        );
        // Harper's dictionary normalizes some dashes internally. Admission is
        // nevertheless decided by the parser: sentence dashes stay separate.
        for text in ["well–posedness", "well—posedness"] {
            let source: Vec<char> = text.chars().collect();
            let mut tokens = PlainEnglish.parse(&source);
            let original = tokens.clone();
            super::merge_reviewed_compounds(&source, &mut tokens);
            assert_eq!(tokens, original, "{text}");
            assert!(tokens.len() > 1);
        }
    }
}
