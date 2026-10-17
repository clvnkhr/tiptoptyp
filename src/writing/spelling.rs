//! Explicit mathematical spellings and compounds, without accepting arbitrary affixes.

use harper_core::{Token, TokenKind, spell::Dictionary};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

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

/// The Typst adapter joins text/quote/text AST triples greedily, so a second
/// possessive sharing the first one's trailing text can remain three tokens.
/// Rejoin only admitted, contiguous prose. Never cross whitespace, markup or a
/// masked source gap, and retain original Unicode scalar coordinates.
pub(super) fn merge_apostrophes(source: &[char], tokens: &mut Vec<Token>) {
    let mut read = 0;
    let mut write = 0;
    while read < tokens.len() {
        let mut end = read + 1;
        if tokens[read].kind.is_word() {
            while end + 1 < tokens.len()
                && tokens[end + 1].kind.is_word()
                && tokens[end - 1].span.end == tokens[end].span.start
                && tokens[end].span.end == tokens[end + 1].span.start
                && tokens[end].span.len() == 1
                && (tokens[end].kind.is_apostrophe() || tokens[end].kind.is_quote())
                && matches!(source[tokens[end].span.start], '\'' | '\u{2019}')
                && source[tokens[end].span.start - 1].is_alphabetic()
                && source[tokens[end].span.end].is_alphabetic()
            {
                end += 2;
            }
        }
        if end > read + 1 {
            tokens[write] = Token::new(
                harper_core::Span::new(tokens[read].span.start, tokens[end - 1].span.end),
                TokenKind::Word(None),
            );
        } else if write != read {
            tokens[write] = tokens[read].clone();
        }
        write += 1;
        read = end;
    }
    tokens.truncate(write);
}

/// A document-local index of contextual proper names. Capitalization alone is
/// not evidence: sentence starts and headings still need spelling checks. A
/// named mathematical object, attribution or coordinated author list supplies
/// evidence, which also covers that name's later sentence-initial/possessive
/// uses. Only spelling lints are suppressed; repetition and grammar still run.
pub(super) struct Names {
    spans: Vec<(usize, usize)>,
}

impl Names {
    pub(super) fn new(source: &[char], tokens: &[Token]) -> Self {
        let candidates: Vec<_> = tokens
            .iter()
            .enumerate()
            .filter(|(_, token)| token.kind.is_word())
            .filter_map(|(index, token)| {
                let word = token.span.get_content(source);
                name_base(word).map(|base| (index, token.span, base))
            })
            .collect();
        let mut names = HashSet::new();
        let mut coordinated = HashMap::<&[char], Vec<&[char]>>::new();
        for &(index, _, base) in &candidates {
            // These reviewed mathematical surnames cover isolated mentions with
            // no local grammatical context. They are case-sensitive names, not
            // lowercase dictionary words or an arbitrary capitalized exemption.
            let reviewed = ["Tao", "Barbato", "Romito"]
                .iter()
                .any(|name| base.iter().copied().eq(name.chars()));
            let before = neighbour(tokens, index, false);
            let after = neighbour(tokens, index, true);
            let attributed = before.is_some_and(|(_, token)| {
                word_is(source, token, &["by", "of", "from", "following"])
            });
            let mathematical_name = before.is_some_and(|(_, token)| token.kind.is_word())
                && after.is_some_and(|(_, token)| {
                    word_is(
                        source,
                        token,
                        &[
                            "theorem",
                            "theorems",
                            "lemma",
                            "lemmas",
                            "inequality",
                            "inequalities",
                            "estimate",
                            "estimates",
                            "equation",
                            "equations",
                            "conjecture",
                            "conjectures",
                            "operator",
                            "operators",
                            "space",
                            "spaces",
                            "method",
                            "methods",
                            "criterion",
                            "criteria",
                            "identity",
                            "identities",
                            "functional",
                            "functionals",
                            "formula",
                            "formulas",
                        ],
                    )
                });
            let dictionary_name = super::WRITING_DICTIONARY
                .get_word_metadata(base)
                .is_some_and(|metadata| metadata.is_proper_noun());
            if reviewed || attributed || mathematical_name || dictionary_name {
                names.insert(base);
            }
            // X and Y / X, Y: propagate evidence from a recognized author to
            // the rest of a list, without treating two capitalized typos as
            // sufficient evidence for each other.
            for direction in [false, true] {
                if let Some((separator_index, separator)) = neighbour(tokens, index, direction)
                    && (word_is(source, separator, &["and"]) || separator.kind.is_comma())
                    && let Some((_, other)) = neighbour(tokens, separator_index, direction)
                    && other.kind.is_word()
                    && let Some(other_base) = name_base(other.span.get_content(source))
                {
                    coordinated.entry(base).or_default().push(other_base);
                }
            }
        }
        let mut pending: Vec<_> = names.iter().copied().collect();
        while let Some(name) = pending.pop() {
            if let Some(others) = coordinated.get(name) {
                for &other in others {
                    if names.insert(other) {
                        pending.push(other);
                    }
                }
            }
        }
        let mut spans: Vec<_> = candidates
            .into_iter()
            .filter(|(_, _, base)| names.contains(base))
            .map(|(_, span, _)| (span.start, span.end))
            .collect();
        if spans.windows(2).any(|pair| pair[0] > pair[1]) {
            spans.sort_unstable();
        }
        spans.dedup();
        Self { spans }
    }

    pub(super) fn contains(&self, start: usize, end: usize) -> bool {
        self.spans.binary_search(&(start, end)).is_ok()
    }
}

/// An optional capitalization exemption, evaluated only for spelling lints.
/// Require the entire word to be alphabetic, begin in uppercase, and contain
/// lowercase letters: titles/PascalCase qualify; acronyms, digits and fragments
/// of identifiers do not. Possessive suffixes retain their original spans.
pub(super) fn capitalized_name_span(source: &[char], start: usize, end: usize) -> bool {
    let Some(word) = source.get(start..end) else {
        return false;
    };
    let word = match word {
        [base @ .., '\'' | '\u{2019}', 's'] => base,
        _ => word,
    };
    word.len() >= 2
        && word[0].is_uppercase()
        && word.iter().all(|ch| ch.is_alphabetic())
        && word.iter().any(|ch| ch.is_lowercase())
        && start
            .checked_sub(1)
            .and_then(|index| source.get(index))
            .is_none_or(|ch| !ch.is_alphanumeric() && *ch != '_')
        && source
            .get(end)
            .is_none_or(|ch| !ch.is_alphanumeric() && *ch != '_')
}

fn name_base(word: &[char]) -> Option<&[char]> {
    let base = match word {
        [base @ .., '\'' | '\u{2019}', 's'] => base,
        _ => word,
    };
    (base.len() >= 2
        && base[0].is_uppercase()
        && base[1..]
            .iter()
            .all(|ch| ch.is_alphabetic() || matches!(ch, '\'' | '\u{2019}'))
        && base[1..].iter().any(|ch| ch.is_lowercase()))
    .then_some(base)
}

fn neighbour(tokens: &[Token], index: usize, forward: bool) -> Option<(usize, &Token)> {
    if forward {
        first_neighbour(tokens.iter().enumerate().skip(index + 1))
    } else {
        first_neighbour(tokens[..index].iter().enumerate().rev())
    }
}

fn first_neighbour<'a>(
    tokens: impl Iterator<Item = (usize, &'a Token)>,
) -> Option<(usize, &'a Token)> {
    let mut line_breaks = 0;
    for (index, token) in tokens {
        match token.kind {
            TokenKind::ParagraphBreak => return None,
            TokenKind::Newline(count) => {
                line_breaks += count;
                if line_breaks >= 2 {
                    return None;
                }
            }
            _ => {}
        }
        if !token.kind.is_whitespace() {
            return Some((index, token));
        }
    }
    None
}

fn word_is(source: &[char], token: &Token, words: &[&str]) -> bool {
    token.kind.is_word()
        && words.iter().any(|word| {
            token
                .span
                .get_content(source)
                .iter()
                .copied()
                .eq(word.chars())
        })
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
    merge_dictionary_compounds(
        source,
        tokens,
        super::WRITING_DICTIONARY.as_ref(),
        *MAX_REVIEWED_COMPOUND_SCALARS,
    );
}

pub(super) fn merge_dictionary_compounds(
    source: &[char],
    tokens: &mut Vec<Token>,
    dictionary: &impl Dictionary,
    max_compound_scalars: usize,
) {
    if max_compound_scalars == 0 {
        return;
    }
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
            && span_end - span_start <= max_compound_scalars
            && dictionary.contains_word(&source[span_start..span_end]);
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
    fn capitalization_exemption_requires_whole_unicode_names() {
        for name in [
            "Leray",
            "MacDonald",
            "Évariste",
            "Zulverton's",
            "Zulverton’s",
        ] {
            let chars: Vec<_> = name.chars().collect();
            assert!(
                super::capitalized_name_span(&chars, 0, chars.len()),
                "{name}"
            );
        }
        for word in [
            "A",
            "ABC",
            "lowerCamelCase",
            "camelCase",
            "name",
            "Name2",
            "Name-Name",
            "Name_Name",
            "Name Name",
            "Name.",
            "O'Neill",
        ] {
            let chars: Vec<_> = word.chars().collect();
            assert!(
                !super::capitalized_name_span(&chars, 0, chars.len()),
                "{word}"
            );
        }
        for source in ["Name2", "2Name", "Name_other", "other_Name"] {
            let chars: Vec<_> = source.chars().collect();
            let start = chars.iter().position(|ch| *ch == 'N').unwrap();
            assert!(
                !super::capitalized_name_span(&chars, start, start + 4),
                "identifier fragments do not qualify: {source}"
            );
        }
    }

    #[test]
    fn possessives_and_mathematician_names_are_not_reported_as_typos() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            for source in [
                "Tao's argument proves the theorem.",
                "Tao’s argument proves the theorem.",
                "The proof follows Tao's argument and Tao's conclusion.",
                "The proof follows Tao’s argument and Tao’s conclusion.",
                "Tao's, Romito's and Barbato's work proves this result.",
                "Barbato's result proves the theorem.",
                "Romito’s result proves the theorem.",
                "Barbato and Romito proved this result.",
                "The proof follows Barbato and Romito.\nBarbato proved the theorem.",
            ] {
                let issues = super::super::check(
                    source,
                    kind,
                    Path::new("fixture"),
                    true,
                    false,
                    WritingLanguage::British,
                );
                assert!(issues.is_empty(), "{kind:?}: {source}: {issues:?}");
            }
        }
    }

    #[test]
    fn repeated_possessives_still_run_grammar_and_unknown_lowercase_words_still_lint() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let issues = super::super::check(
                "Tao's Tao's theorem is misspelld.\nBarbato Barbato proved the result.\nRomito romito proved the result.",
                kind,
                Path::new("fixture"),
                true,
                false,
                WritingLanguage::British,
            );
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.line() == Some(1) && issue.message.contains("repeat")),
                "{kind:?}: {issues:?}"
            );
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.line() == Some(1) && issue.message.contains("misspelld")),
                "{kind:?}: {issues:?}"
            );
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.line() == Some(2) && issue.message.contains("repeat")),
                "{kind:?}: {issues:?}"
            );
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.line() == Some(3) && issue.message.contains("romito")),
                "{kind:?}: {issues:?}"
            );
            assert!(
                issues
                    .iter()
                    .all(|issue| !issue.message.contains("spell `s`")),
                "{kind:?}: {issues:?}"
            );
        }
    }

    #[test]
    fn names_need_context_not_just_capitalization_or_repetition() {
        for (source, accepted) in [
            ("Thiss is misspelld. Thiss is still wrong.", false),
            ("Thiss Heading", false),
            ("Thiss theorem is wrong.", false),
            ("A proof is known.\n\nThiss is wrong.", false),
            (
                "According to a result by Zulverton. Zulverton proved this.",
                true,
            ),
            ("The Quenly theorem is useful. Quenly proved this.", true),
            (
                "Zulverton and Quenly proved this. Zulverton proved this.",
                false,
            ),
            (
                "A result by Zulverton and Quenly. Quenly proved this.",
                true,
            ),
            ("Thiss and Thatt are misspelled.", false),
            ("by\n\nZulverton", false),
            ("by\n  \nZulverton", false),
            ("A result by\n  Zulverton", true),
            ("The\n\nZulverton theorem.", false),
            ("Zulverton, Quenly and Romito proved this.", true),
        ] {
            let chars: Vec<_> = source.chars().collect();
            let tokens = PlainEnglish.parse(&chars);
            let names = super::Names::new(&chars, &tokens);
            for token in tokens.iter().filter(|token| {
                token.kind.is_word()
                    && matches!(
                        token.get_str(&chars).as_str(),
                        "Thiss" | "Thatt" | "Zulverton" | "Quenly"
                    )
            }) {
                assert_eq!(
                    names.contains(token.span.start, token.span.end),
                    accepted,
                    "{source}: {:?}",
                    token.get_str(&chars)
                );
            }
            assert!(!names.contains(chars.len(), chars.len()));
        }
    }

    #[test]
    fn apostrophe_repair_is_in_place_and_never_joins_gaps_code_or_quotes() {
        use harper_core::{Punctuation, Span, Token};
        for apostrophe in ['\'', '\u{2019}'] {
            let chars: Vec<_> = format!("Tao{apostrophe}s").chars().collect();
            let mut tokens = vec![
                Token::new(Span::new(0, 3), TokenKind::Word(None)),
                Token::new(
                    Span::new(3, 4),
                    TokenKind::Punctuation(Punctuation::Apostrophe),
                ),
                Token::new(Span::new(4, 5), TokenKind::Word(None)),
            ];
            let allocation = tokens.as_ptr();
            let capacity = tokens.capacity();
            super::merge_apostrophes(&chars, &mut tokens);
            assert_eq!(tokens.as_ptr(), allocation);
            assert_eq!(tokens.capacity(), capacity);
            assert_eq!(tokens.len(), 1);
            assert_eq!(tokens[0].span, Span::new(0, chars.len()));
            assert_eq!(tokens[0].get_str(&chars), chars.iter().collect::<String>());
        }
        for source in [
            "Tao 's", "Tao' s", "Tao'", "'s'", "Tao\"s", "Tao*'s", "Tao's",
        ] {
            let chars: Vec<_> = source.chars().collect();
            let mut tokens = PlainEnglish.parse(&chars);
            let original = tokens.clone();
            super::merge_apostrophes(&chars, &mut tokens);
            assert_eq!(tokens, original, "{source}");
        }
        let chars: Vec<_> = "Tao's".chars().collect();
        let mut tokens = vec![
            Token::new(Span::new(0, 3), TokenKind::Word(None)),
            Token::new(Span::new(3, 4), TokenKind::Unlintable),
            Token::new(Span::new(4, 5), TokenKind::Word(None)),
        ];
        let original = tokens.clone();
        super::merge_apostrophes(&chars, &mut tokens);
        assert_eq!(tokens, original);
    }

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
