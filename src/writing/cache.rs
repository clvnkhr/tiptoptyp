//! Cache semantic paragraphs after the complete source parser has established
//! math/code boundaries. A changed line can affect arbitrarily later syntax.
#[cfg(test)]
mod probe;

use super::*;
use harper_core::{Token, TokenKind, parsers::Parser};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
};

const MAX_BLOCKS: usize = 1024;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;

pub(super) struct Replay<'a>(pub(super) &'a [Token]);
impl Parser for Replay<'_> {
    fn parse(&self, _: &[char]) -> Vec<Token> {
        self.0.to_vec()
    }
}
struct Block {
    text: Box<[char]>,
    tokens: Vec<Token>,
    lints: Vec<NamedLint>,
    bytes: usize,
    used: u64,
}
#[derive(Default)]
pub(crate) struct Checker {
    context: Option<(DocumentKind, Dialect)>,
    blocks: HashMap<u64, Block>,
    bytes: usize,
    clock: u64,
    dictionary_source: String,
    personal_words: Vec<String>,
    dictionary: Option<Arc<MergedDictionary>>,
    max_personal_compound_scalars: usize,
    #[cfg(test)]
    stats: (usize, usize),
}
impl Checker {
    #[cfg(test)]
    pub(crate) fn check(
        &mut self,
        source: &str,
        kind: DocumentKind,
        path: &Path,
        grammar: bool,
        unicode: bool,
        language: crate::settings::WritingLanguage,
    ) -> Vec<Diagnostic> {
        self.check_with_options(
            source,
            kind,
            path,
            grammar,
            unicode,
            language,
            Options::default(),
        )
    }

    fn configure_dictionary(&mut self, source: &str) {
        if self.dictionary.is_some() && self.dictionary_source == source {
            return;
        }
        self.dictionary_source.clear();
        self.dictionary_source.push_str(source);
        // A malformed loaded configuration cannot send an unbounded word to
        // Harper's fuzzy matcher. Reject the complete list, never a prefix.
        let accepted = if validate_dictionary(source).is_ok() {
            source
        } else {
            ""
        };
        let mut words: Vec<_> = accepted
            .lines()
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .flat_map(spelling::dictionary_variants)
            .map(|word| word.into_owned())
            .collect();
        words.sort_unstable();
        words.dedup();
        if self.dictionary.is_some() && words == self.personal_words {
            return;
        }
        self.blocks.clear();
        self.bytes = 0;
        self.max_personal_compound_scalars = words
            .iter()
            .filter(|word| word.contains(['-', '\u{2010}', '\u{2011}']))
            .map(|word| word.chars().count())
            .max()
            .unwrap_or(0);
        self.dictionary = Some(if words.is_empty() {
            Arc::clone(&WRITING_DICTIONARY)
        } else {
            use harper_core::spell::Dictionary;
            let entries = words
                .iter()
                .map(|word| {
                    let mut metadata = WRITING_DICTIONARY
                        .get_word_metadata_str(word)
                        .map(|metadata| metadata.into_owned())
                        .unwrap_or_default();
                    // An explicitly accepted spelling overrides dialect limits.
                    // MergedDictionary ORs dialect flags, so empty/default flags
                    // would retain the curated restriction instead of lifting it.
                    metadata.dialects = harper_core::DialectFlags::all();
                    (word.chars().collect(), metadata)
                })
                .collect();
            // Clone only the dictionary's Arc children, retaining curated metadata.
            let mut dictionary = WRITING_DICTIONARY.as_ref().clone();
            dictionary.add_dictionary(Arc::new(FstDictionary::new(entries)));
            Arc::new(dictionary)
        });
        self.personal_words = words;
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn check_with_options(
        &mut self,
        source: &str,
        kind: DocumentKind,
        path: &Path,
        grammar: bool,
        unicode: bool,
        language: crate::settings::WritingLanguage,
        options: Options<'_>,
    ) -> Vec<Diagnostic> {
        #[cfg(test)]
        {
            self.stats = (0, 0);
        }
        let characters: Vec<_> = source.chars().collect();
        let mut tokens = Vec::new();
        let mut lints = Vec::new();
        let enabled = grammar && matches!(kind, DocumentKind::Typst | DocumentKind::Tex);
        let parsed = (enabled && kind == DocumentKind::Typst)
            .then(|| typst_prose::TypstProse::parse_with_language(&characters));
        let dialect = enabled
            .then(|| {
                let lang = parsed
                    .as_ref()
                    .and_then(|parsed| parsed.language.lang.as_deref());
                let region = parsed
                    .as_ref()
                    .and_then(|parsed| parsed.language.region.as_deref());
                resolve_dialect(language, lang, region)
            })
            .flatten();
        let context = dialect.map(|dialect| (kind, dialect));
        if self.context != context {
            self.blocks.clear();
            self.bytes = 0;
            self.context = context;
        }
        if let Some(dialect) = dialect {
            self.configure_dictionary(options.dictionary);
            let dictionary = Arc::clone(self.dictionary.as_ref().unwrap());
            tokens = if let Some(parsed) = parsed {
                let mut tokens = parsed.tokens;
                normalize_tokens(&characters, &mut tokens);
                tokens
            } else {
                if kind == DocumentKind::Tex
                    && let Some(rules) = options.tex_prose_rules
                {
                    let mut tokens =
                        tex_prose::TexProse::parse_with_rules(&characters, rules, false);
                    normalize_tokens(&characters, &mut tokens);
                    tokens
                } else {
                    Prose(kind).parse(&characters)
                }
            };
            spelling::merge_dictionary_compounds(
                &characters,
                &mut tokens,
                dictionary.as_ref(),
                self.max_personal_compound_scalars,
            );
            // Harper's own expression LRUs key only source/config, not token
            // eligibility. Recreate once per request with misses, so moving the
            // same text into/out of math/code cannot reuse stale grammar rules.
            let mut linter = None;
            for (token_range, source_range) in paragraph_ranges(&tokens, characters.len()) {
                let (token_start, end) = (token_range.start, token_range.end);
                let (char_start, char_end) = (source_range.start, source_range.end);
                let mut relative = tokens[token_start..end].to_vec();
                for token in &mut relative {
                    token.span.pull_by(char_start);
                }
                let text = &characters[char_start..char_end];
                let mut hash = DefaultHasher::new();
                text.hash(&mut hash);
                for token in &relative {
                    token.span.start.hash(&mut hash);
                    token.span.end.hash(&mut hash);
                    token.kind.hash(&mut hash);
                }
                let key = hash.finish();
                self.clock = self.clock.saturating_add(1);
                let mut result = if let Some(block) = self
                    .blocks
                    .get_mut(&key)
                    .filter(|block| block.text.as_ref() == text && block.tokens == relative)
                {
                    block.used = self.clock;
                    #[cfg(test)]
                    {
                        self.stats.0 += 1;
                    }
                    block.lints.clone()
                } else {
                    #[cfg(test)]
                    {
                        self.stats.1 += 1;
                    }
                    let document = Document::new_from_chars(
                        text.to_vec().into(),
                        &Replay(&relative),
                        dictionary.as_ref(),
                    );
                    let linter = linter.get_or_insert_with(|| {
                        LintGroup::new_curated(Arc::clone(&dictionary), dialect)
                    });
                    let result = named_lints(linter, &document);
                    let bytes = std::mem::size_of_val(text)
                        + relative.len() * std::mem::size_of::<Token>()
                        + result
                            .iter()
                            .map(|lint| {
                                std::mem::size_of::<NamedLint>()
                                    + lint.rule.len()
                                    + lint.message.len()
                                    + lint
                                        .suggestions
                                        .iter()
                                        .map(|suggestion| {
                                            std::mem::size_of_val(suggestion) + match suggestion {
                                                harper_core::linting::Suggestion::ReplaceWith(
                                                    chars,
                                                )
                                                | harper_core::linting::Suggestion::InsertAfter(
                                                    chars,
                                                ) => std::mem::size_of_val(chars.as_slice()),
                                                harper_core::linting::Suggestion::Remove => 0,
                                            }
                                        })
                                        .sum::<usize>()
                            })
                            .sum::<usize>();
                    if bytes <= MAX_CACHE_BYTES {
                        if let Some(old) = self.blocks.remove(&key) {
                            self.bytes -= old.bytes;
                        }
                        while self.blocks.len() >= MAX_BLOCKS
                            || self.bytes + bytes > MAX_CACHE_BYTES
                        {
                            let Some(oldest) = self
                                .blocks
                                .iter()
                                .min_by_key(|(_, block)| block.used)
                                .map(|(&key, _)| key)
                            else {
                                break;
                            };
                            self.bytes -= self.blocks.remove(&oldest).unwrap().bytes;
                        }
                        self.bytes += bytes;
                        self.blocks.insert(
                            key,
                            Block {
                                text: text.into(),
                                tokens: relative,
                                lints: result.clone(),
                                bytes,
                                used: self.clock,
                            },
                        );
                    }
                    result
                };
                for lint in &mut result {
                    lint.span.push_by(char_start);
                }
                lints.extend(result);
            }
        }
        diagnostics(
            &characters,
            &tokens,
            lints,
            path,
            unicode,
            options.ignore_capitalized_names,
        )
    }
    #[cfg(test)]
    pub(super) fn cache_stats(&self) -> (usize, usize) {
        self.stats
    }
}

/// TeX structural markers can overlap a wide masked paragraph-break token.
/// Coarsen those blocks rather than cutting/rebasing through a backwards span.
fn paragraph_ranges(
    tokens: &[Token],
    source_len: usize,
) -> Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let mut suffix_min = vec![source_len; tokens.len() + 1];
    for index in (0..tokens.len()).rev() {
        suffix_min[index] = suffix_min[index + 1].min(tokens[index].span.start);
    }
    let (mut token_start, mut char_start, mut prefix_end) = (0, 0, 0);
    let mut ranges = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        prefix_end = prefix_end.max(token.span.end);
        let end = index + 1;
        let boundary =
            token.kind.is_paragraph_break() || matches!(token.kind,TokenKind::Newline(n) if n>=2);
        if end == tokens.len()
            || (boundary && token.span.end == prefix_end && prefix_end <= suffix_min[end])
        {
            let char_end = if end == tokens.len() {
                source_len
            } else {
                prefix_end
            };
            ranges.push((token_start..end, char_start..char_end));
            token_start = end;
            char_start = char_end;
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::WritingLanguage;
    fn check(checker: &mut Checker, source: &str, kind: DocumentKind) -> Vec<Diagnostic> {
        let actual = checker.check(
            source,
            kind,
            Path::new("fixture"),
            true,
            true,
            WritingLanguage::Auto,
        );
        assert_eq!(
            actual,
            check_uncached(
                source,
                kind,
                Path::new("fixture"),
                true,
                true,
                WritingLanguage::Auto
            ),
            "{kind:?}: {source}"
        );
        actual
    }
    #[test]
    fn changing_argument_roles_cannot_reuse_opaque_paragraph_results() {
        use crate::writing::prose_rules::{TexArgumentRole, TexCommandRule, TexProseRules};
        let source = r"The \mycomment{qzorthal} result is clear.";
        let mut rules = TexProseRules::default();
        rules.commands.push(TexCommandRule {
            name: "mycomment".into(),
            arguments: vec![TexArgumentRole::Literal],
            optional_arguments: Vec::new(),
        });
        let mut checker = Checker::default();
        let mut run = |rules: &TexProseRules| {
            checker.check_with_options(
                source,
                DocumentKind::Tex,
                Path::new("roles.tex"),
                true,
                false,
                WritingLanguage::British,
                Options {
                    tex_prose_rules: Some(rules),
                    ..Options::default()
                },
            )
        };
        assert!(
            run(&rules)
                .iter()
                .all(|issue| !issue.message.contains("qzorthal"))
        );
        rules.commands.last_mut().unwrap().arguments = vec![TexArgumentRole::InlineProse];
        let visible = run(&rules);
        let word = visible
            .iter()
            .find_map(|issue| {
                issue
                    .correction
                    .as_ref()
                    .filter(|fix| fix.original == "qzorthal")
            })
            .expect("custom prose is linted after a role change");
        let start = source[..source.find("qzorthal").unwrap()].chars().count();
        assert_eq!(word.range, start..start + 8);
        assert_eq!(
            run(&rules),
            visible,
            "cache hits preserve source ranges and suggestions"
        );
        rules.commands.last_mut().unwrap().arguments = vec![TexArgumentRole::Literal];
        assert!(
            run(&rules)
                .iter()
                .all(|issue| !issue.message.contains("qzorthal"))
        );
    }

    #[test]
    fn inline_typst_formatting_does_not_hide_repetitions_or_invent_capitalization() {
        let mut checker = Checker::default();
        for wrapper in ["emph", "strong", "text"] {
            for clean in [
                format!("These #{wrapper}[estimates] are uniform."),
                format!("These\n#{wrapper}[\n  estimates\n]\nare uniform."),
                format!("The\n#{wrapper}[]\ntheorem holds."),
            ] {
                let issues = check(&mut checker, &clean, DocumentKind::Typst);
                assert!(issues.is_empty(), "{clean}: {issues:?}");
            }
            let repeated = format!("This is the #{wrapper}[the] result.");
            let lints = check(&mut checker, &repeated, DocumentKind::Typst);
            assert!(
                lints.iter().any(|lint| lint.message.contains("repeat")),
                "{repeated}: {lints:?}"
            );
        }
    }

    #[test]
    fn inline_typst_formatting_repetitions_keep_their_source_locations() {
        for wrapper in ["emph", "strong", "text"] {
            for source in [
                format!("This is the #{wrapper}[the] result."),
                format!("This is #{wrapper}[the] the result."),
                format!("This is #{wrapper}[the the] result."),
            ] {
                let mut checker = Checker::default();
                let issues = check(&mut checker, &source, DocumentKind::Typst);
                let first_the = source.find("the").unwrap();
                let expected = DiagnosticLocation {
                    line: 1,
                    column: source[..first_the].chars().count() + 1,
                };
                assert!(
                    issues.iter().any(|issue| {
                        issue.message.contains("repeat") && issue.location == Some(expected)
                    }),
                    "{source}: {issues:?}"
                );
            }
        }
    }

    #[test]
    fn inline_typst_formatting_cache_reuses_paragraphs_and_rebases_unicode_locations() {
        for wrapper in ["emph", "strong", "text"] {
            let mut checker = Checker::default();
            // The curly apostrophe precedes the formatted word on its own line,
            // so both global and line-local UTF-8 offsets differ from scalars.
            let source = format!(
                "The first estimate holds.\n\nTao’s argument gives these #{wrapper}[estimates].\n\nThe final estimate holds."
            );
            assert!(check(&mut checker, &source, DocumentKind::Typst).is_empty());
            let characters: Vec<_> = source.chars().collect();
            let tokens = Prose(DocumentKind::Typst).parse(&characters);
            let paragraphs = paragraph_ranges(&tokens, characters.len());
            let prose_paragraphs = paragraphs
                .iter()
                .filter(|(range, _)| {
                    tokens[range.clone()]
                        .iter()
                        .any(|token| token.kind.is_word())
                })
                .count();
            assert_eq!(prose_paragraphs, 3, "{source}: {tokens:?}");
            for (range, span) in &paragraphs {
                assert!(
                    tokens[range.clone()].iter().all(|token| {
                        span.start <= token.span.start && token.span.end <= span.end
                    }),
                    "{source}: paragraph {span:?} must own its tokens"
                );
            }
            assert!(
                tokens
                    .windows(2)
                    .all(|pair| pair[0].span.end <= pair[1].span.start)
            );

            assert!(check(&mut checker, &source, DocumentKind::Typst).is_empty());
            assert_eq!(checker.stats.1, 0, "unchanged formatted prose is cached");
            assert!(checker.stats.0 >= 3);

            let edited = source.replace("estimates", "estmates");
            let expected_location = |text: &str| {
                let before = &text[..text.find("estmates").unwrap()];
                DiagnosticLocation {
                    line: before.chars().filter(|&ch| ch == '\n').count() + 1,
                    column: before.rsplit('\n').next().unwrap().chars().count() + 1,
                }
            };
            let issues = check(&mut checker, &edited, DocumentKind::Typst);
            assert_eq!(issues.len(), 1, "{edited}: {issues:?}");
            assert_eq!(issues[0].location, Some(expected_location(&edited)));
            assert_eq!(
                issues[0].source,
                DiagnosticSource::File(Path::new("fixture").to_owned())
            );
            assert_eq!(checker.stats.1, 1, "only the edited paragraph is rechecked");
            assert!(checker.stats.0 >= 2);

            // A new line joins the first paragraph. The unchanged formatted
            // paragraph must be a cache hit with rebased source coordinates.
            let shifted = format!("Tao’s theorem applies.\n{edited}");
            let issues = check(&mut checker, &shifted, DocumentKind::Typst);
            assert_eq!(issues.len(), 1, "{shifted}: {issues:?}");
            assert_eq!(issues[0].location, Some(expected_location(&shifted)));
            assert_eq!(checker.stats.1, 1);
            assert!(checker.stats.0 >= 2);
            check(&mut checker, &shifted, DocumentKind::Typst);
            assert_eq!(checker.stats.1, 0);

            // Restoring the old body can reuse its earlier clean cache entry.
            assert!(check(&mut checker, &source, DocumentKind::Typst).is_empty());
            assert_eq!(checker.stats.1, 0);
        }
    }

    #[test]
    fn unchanged_paragraphs_are_reused_and_one_edit_only_rechecks_its_block() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let mut checker = Checker::default();
            let source = "The first estimate is stable.\n\nThis is the the result.\n\nThe last sentence is misspelld.";
            let initial = check(&mut checker, source, kind);
            assert!(initial.len() >= 2);
            assert!(checker.stats.1 >= 3);
            check(&mut checker, source, kind);
            assert!(checker.stats.0 >= 3);
            assert_eq!(checker.stats.1, 0);
            let edited = source.replace("the the", "the");
            let current = check(&mut checker, &edited, kind);
            assert!(current.len() < initial.len());
            assert_eq!(checker.stats.1, 1);
            assert!(checker.stats.0 >= 2);
            // An earlier inserted Unicode line moves all later locations.
            check(&mut checker, &format!("A new α line.\n{edited}"), kind);
            assert!(checker.stats.0 >= 2);
            assert_eq!(checker.stats.1, 1);
        }
    }
    #[test]
    fn display_math_and_wrapped_prose_keep_sentence_context() {
        for (kind, source) in [
            (
                DocumentKind::Typst,
                "We obtain\n$\n x = y,\n\n z = w\n$\nand and the result follows.\n\nA different misspelld result.",
            ),
            (
                DocumentKind::Tex,
                "We obtain\n\\[\n x = y,\n\n z = w\n\\]\nand and the result follows.\n\nA different misspelld result.",
            ),
            (
                DocumentKind::Typst,
                "He said \"This is\na quoted sentence.\" Then this is the the result.\n\nAnother paragraph follows.",
            ),
        ] {
            let mut checker = Checker::default();
            let initial = check(&mut checker, source, kind);
            assert!(
                initial.iter().any(|d| d.message.contains("repeat")),
                "{initial:?}"
            );
            check(&mut checker, source, kind);
            assert_eq!(checker.stats.1, 0);
            check(&mut checker, &source.replace("and and", "and"), kind);
        }
    }
    #[test]
    fn unchanged_text_is_invalidated_when_surrounding_syntax_hides_it() {
        for (kind, opener, closer) in [
            (DocumentKind::Typst, "```typ\n", "\n```"),
            (
                DocumentKind::Tex,
                "\\begin{verbatim}\n",
                "\n\\end{verbatim}",
            ),
        ] {
            let mut checker = Checker::default();
            let source = "This is the the misspelld result.\n\nAnother wrongg result.";
            assert!(check(&mut checker, source, kind).len() >= 2);
            assert!(check(&mut checker, &format!("{opener}{source}{closer}"), kind).is_empty());
            assert!(check(&mut checker, source, kind).len() >= 2);
            check(&mut checker, &format!("{opener}{source}"), kind);
        }
    }
    #[test]
    fn current_document_name_evidence_filters_cached_spelling_results() {
        let mut checker = Checker::default();
        let source = "A short introduction.\n\nZulverton proves this result.";
        assert!(
            check(&mut checker, source, DocumentKind::Typst)
                .iter()
                .any(|d| d.location == Some(DiagnosticLocation { line: 3, column: 1 }))
        );
        let named = source.replace(
            "A short introduction.",
            "The result was proved by Zulverton.",
        );
        assert!(
            check(&mut checker, &named, DocumentKind::Typst)
                .iter()
                .all(|d| d.location != Some(DiagnosticLocation { line: 3, column: 1 }))
        );
        assert!(
            checker.stats.0 > 0,
            "name filtering must not require invalidating unchanged paragraph grammar"
        );
    }
    #[test]
    fn dialect_flags_and_paths_cannot_reuse_incompatible_results() {
        let mut checker = Checker::default();
        let source = "The colour remains clear.\n\nA misspelld result.";
        for language in [
            WritingLanguage::British,
            WritingLanguage::American,
            WritingLanguage::Auto,
        ] {
            let actual = checker.check(
                source,
                DocumentKind::Typst,
                Path::new("new-path.typ"),
                true,
                false,
                language,
            );
            assert_eq!(
                actual,
                check_uncached(
                    source,
                    DocumentKind::Typst,
                    Path::new("new-path.typ"),
                    true,
                    false,
                    language
                )
            );
            assert!(
                actual
                    .iter()
                    .all(|d| d.source == DiagnosticSource::File("new-path.typ".into()))
            );
        }
        let disabled = "#set text(lang: \"de\")\n\nA misspelld result.";
        assert!(check(&mut checker, disabled, DocumentKind::Typst).is_empty());
        let unicode = "A Latin lookalike аlpha and a zero\u{200b}width character.";
        assert_eq!(
            checker.check(
                unicode,
                DocumentKind::Typst,
                Path::new("unicode.typ"),
                false,
                true,
                WritingLanguage::Auto
            ),
            check_uncached(
                unicode,
                DocumentKind::Typst,
                Path::new("unicode.typ"),
                false,
                true,
                WritingLanguage::Auto
            )
        );
    }
    #[test]
    fn tex_overlapping_structural_boundaries_never_cut_through_a_span() {
        let mut checker = Checker::default();
        for source in [
            "This is is a test.\n\n\\section{The mathematical result}",
            "\\documentclass{article}\n\\begin{document}\nA misspelld result.\n\n\\section{Next}\nThis is the the proof.\n\\end{document}",
        ] {
            check(&mut checker, source, DocumentKind::Tex);
            check(&mut checker, source, DocumentKind::Tex);
            assert_eq!(checker.stats.1, 0);
            let chars: Vec<_> = source.chars().collect();
            let tokens = Prose(DocumentKind::Tex).parse(&chars);
            for (range, span) in paragraph_ranges(&tokens, chars.len()) {
                assert!(
                    tokens[range]
                        .iter()
                        .all(|token| token.span.start >= span.start && token.span.end <= span.end)
                );
            }
        }
    }

    #[test]
    fn paragraph_cache_is_bounded_and_eviction_preserves_results() {
        let mut checker = Checker::default();
        let source = (0..MAX_BLOCKS + 8)
            .map(|index| format!("The value is {index}.\n\n"))
            .collect::<String>();
        check(&mut checker, &source, DocumentKind::Tex);
        assert!(checker.blocks.len() <= MAX_BLOCKS);
        assert!(checker.bytes <= MAX_CACHE_BYTES);
        check(&mut checker, "This is the the result.", DocumentKind::Tex);
        assert!(checker.blocks.len() <= MAX_BLOCKS);
    }

    fn configured(
        checker: &mut Checker,
        source: &str,
        kind: DocumentKind,
        options: Options<'_>,
    ) -> Vec<Diagnostic> {
        checker.check_with_options(
            source,
            kind,
            Path::new("fixture"),
            true,
            true,
            WritingLanguage::British,
            options,
        )
    }

    #[test]
    fn personal_dictionary_add_remove_rebuilds_only_for_changed_words_in_both_languages() {
        use harper_core::spell::Dictionary;
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let mut checker = Checker::default();
            let source = "The zorbaflume is clear.\n\nThis is the the result.";
            let baseline = configured(&mut checker, source, kind, Options::default());
            assert!(
                baseline.iter().any(|d| d.message.contains("zorbaflume")),
                "{kind:?}: {baseline:?}"
            );
            let options = Options {
                dictionary: "zorbaflume\nflurn-blurn",
                ..Options::default()
            };
            let added = configured(&mut checker, source, kind, options);
            assert!(
                added.iter().all(|d| !d.message.contains("zorbaflume")),
                "{kind:?}: {added:?}"
            );
            assert!(
                added.iter().any(|d| d.message.contains("repeat")),
                "grammar remains enabled: {added:?}"
            );
            assert!(
                checker.stats.1 > 0,
                "dictionary changes invalidate raw paragraph results"
            );
            let dictionary = Arc::clone(checker.dictionary.as_ref().unwrap());
            assert!(dictionary.contains_word_str("zorbaflume"));
            let suggestions = harper_core::spell::suggest_correct_spelling(
                &"zorbafluma".chars().collect::<Vec<_>>(),
                200,
                2,
                dictionary.as_ref(),
            );
            assert!(
                suggestions
                    .iter()
                    .any(|suggestion| suggestion.iter().copied().eq("zorbaflume".chars())),
                "personal words participate in suggestions"
            );
            assert!(
                configured(&mut checker, "The flurn-blurn is clear.", kind, options)
                    .iter()
                    .all(|d| !d.message.contains("flurn") && !d.message.contains("blurn"))
            );
            configured(&mut checker, source, kind, options);
            assert_eq!(checker.stats.1, 0);
            let reordered = Options {
                dictionary: "\n flurn-blurn \nzorbaflume\nzorbaflume\n",
                ..Options::default()
            };
            assert_eq!(configured(&mut checker, source, kind, reordered), added);
            assert_eq!(
                checker.stats.1, 0,
                "formatting-only changes retain cached paragraphs"
            );
            assert!(Arc::ptr_eq(
                &dictionary,
                checker.dictionary.as_ref().unwrap()
            ));
            assert_eq!(
                configured(&mut checker, source, kind, Options::default()),
                baseline
            );
            assert!(
                checker.stats.1 > 0,
                "removing a word restores spelling checks"
            );
        }
    }

    #[test]
    fn capitalized_name_toggle_filters_cached_spelling_but_preserves_grammar_and_unicode() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let mut checker = Checker::default();
            let source = "Zulverton Zulverton proves this.\n\nMacZulverton's and Ézulverton’s results use zulverton, lowerCamelZulverton and ZULVERTON.\n\nA zero\u{200b}width value.";
            let baseline = configured(&mut checker, source, kind, Options::default());
            assert!(
                baseline
                    .iter()
                    .any(|d| d.location == Some(DiagnosticLocation { line: 1, column: 1 })),
                "{kind:?}: {baseline:?}"
            );
            let filtered = configured(
                &mut checker,
                source,
                kind,
                Options {
                    ignore_capitalized_names: true,
                    ..Options::default()
                },
            );
            assert!(
                filtered
                    .iter()
                    .filter(|d| d.location == Some(DiagnosticLocation { line: 1, column: 1 }))
                    .all(|d| d.message.contains("repeat")),
                "spelling is removed while repetition at the same location remains: {kind:?}: {filtered:?}"
            );
            assert!(
                filtered.iter().any(|d| d.message.contains("repeat")),
                "repetition is not a spelling lint: {filtered:?}"
            );
            let location_of = |word: &str| {
                let start = source.rfind(word).unwrap();
                let before = &source[..start];
                DiagnosticLocation {
                    line: before.chars().filter(|ch| *ch == '\n').count() + 1,
                    column: before.rsplit('\n').next().unwrap().chars().count() + 1,
                }
            };
            for word in ["zulverton", "lowerCamelZulverton", "ZULVERTON"] {
                assert!(
                    filtered
                        .iter()
                        .any(|d| d.location == Some(location_of(word))),
                    "{kind:?}: {word}: {filtered:?}"
                );
            }
            for name in ["MacZulverton", "Ézulverton"] {
                assert!(
                    baseline
                        .iter()
                        .any(|d| d.location == Some(location_of(name))),
                    "the possessive fixture must originally be flagged: {kind:?}: {baseline:?}"
                );
                assert!(
                    filtered
                        .iter()
                        .all(|d| d.location != Some(location_of(name))),
                    "possessive names are exempt: {kind:?}: {filtered:?}"
                );
            }
            let unicode = |issues: &[Diagnostic]| {
                issues
                    .iter()
                    .filter(|d| d.provider.as_deref() == Some("Unicode"))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert!(!unicode(&filtered).is_empty());
            assert_eq!(unicode(&filtered), unicode(&baseline));
            assert_eq!(
                checker.stats.1, 0,
                "the toggle filters already-cached raw lints"
            );
            assert_eq!(
                configured(&mut checker, source, kind, Options::default()),
                baseline
            );
            assert_eq!(checker.stats.1, 0);
        }
    }
    #[test]
    fn invalid_dictionary_reverts_to_base_without_accepting_a_valid_prefix() {
        let mut checker = Checker::default();
        let source = "The qzorthal is clear.";
        let baseline = configured(
            &mut checker,
            source,
            DocumentKind::Typst,
            Options::default(),
        );
        assert!(!baseline.is_empty());
        assert!(
            configured(
                &mut checker,
                source,
                DocumentKind::Typst,
                Options {
                    dictionary: "qzorthal",
                    ..Options::default()
                }
            )
            .is_empty()
        );
        assert_eq!(
            configured(
                &mut checker,
                source,
                DocumentKind::Typst,
                Options {
                    dictionary: "qzorthal\ntwo words",
                    ..Options::default()
                }
            ),
            baseline
        );
    }

    #[test]
    fn settings_journey_fixture_keeps_distinct_spelling_and_repetition_locations() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let mut checker = Checker::default();
            let source = "We inspect qzorthal today.\nWe inspect QzorNim today.\nThis sentence contains a misspelld word.\nThis is the the simple result.\n";
            let baseline = configured(&mut checker, source, kind, Options::default());
            let positions = |issues: &[Diagnostic]| {
                issues
                    .iter()
                    .map(|d| d.location.unwrap())
                    .collect::<Vec<_>>()
            };
            let expected = [(1, 12), (2, 12), (3, 26), (4, 9)]
                .map(|(line, column)| DiagnosticLocation { line, column });
            assert_eq!(positions(&baseline), expected, "{kind:?}: {baseline:?}");
            let dictionary = configured(
                &mut checker,
                source,
                kind,
                Options {
                    dictionary: "qzorthal",
                    ..Options::default()
                },
            );
            assert_eq!(positions(&dictionary), expected[1..]);
            let names = configured(
                &mut checker,
                source,
                kind,
                Options {
                    dictionary: "qzorthal",
                    ignore_capitalized_names: true,
                    ..Options::default()
                },
            );
            assert_eq!(positions(&names), expected[2..]);
        }
    }
    #[test]
    fn disabled_or_unsupported_writing_language_does_not_build_a_dictionary() {
        let mut checker = Checker::default();
        for (source, grammar) in [
            ("#set text(lang: \"uk\")\nA qzorthal result.", true),
            ("A qzorthal result.", false),
        ] {
            assert!(
                checker
                    .check_with_options(
                        source,
                        DocumentKind::Typst,
                        Path::new("fixture.typ"),
                        grammar,
                        false,
                        WritingLanguage::Auto,
                        Options {
                            dictionary: "qzorthal",
                            ignore_capitalized_names: true,
                            ..Options::default()
                        }
                    )
                    .is_empty()
            );
            assert!(
                checker.dictionary.is_none(),
                "inactive Harper must not initialize or build its large dictionary"
            );
            assert!(checker.blocks.is_empty());
        }
    }
    #[test]
    fn personal_dictionary_accepts_other_dialects_without_losing_grammar_metadata() {
        use harper_core::spell::Dictionary;
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            for (language, dialect, word) in [
                (WritingLanguage::British, Dialect::British, "color"),
                (WritingLanguage::American, Dialect::American, "colour"),
            ] {
                let mut checker = Checker::default();
                let source = format!("The {word} is clear.");
                let check = |checker: &mut Checker, dictionary| {
                    checker.check_with_options(
                        &source,
                        kind,
                        Path::new("fixture"),
                        true,
                        false,
                        language,
                        Options {
                            dictionary,
                            ..Options::default()
                        },
                    )
                };
                let baseline = check(&mut checker, "");
                assert!(
                    !baseline.is_empty(),
                    "wrong-dialect fixture must be flagged: {kind:?}: {language:?}"
                );
                assert!(
                    check(&mut checker, word).is_empty(),
                    "explicit words are accepted regardless of selected dialect: {kind:?}: {language:?}"
                );
                let actual = checker
                    .dictionary
                    .as_ref()
                    .unwrap()
                    .get_word_metadata_str(word)
                    .unwrap()
                    .into_owned();
                let mut expected = WRITING_DICTIONARY
                    .get_word_metadata_str(word)
                    .unwrap()
                    .into_owned();
                expected.dialects = harper_core::DialectFlags::all();
                assert_eq!(
                    actual, expected,
                    "existing grammatical metadata is retained"
                );
                assert!(actual.dialects.is_dialect_enabled(dialect));
                assert_eq!(
                    check(&mut checker, ""),
                    baseline,
                    "removal restores the selected dialect restriction"
                );
            }
        }
    }
}
