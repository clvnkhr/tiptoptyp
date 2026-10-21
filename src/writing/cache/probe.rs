//! Explicit developer export of the actual cached Harper input. This module is
//! absent from app builds and performs I/O only in the ignored opt-in test.
use super::*;
use harper_core::TokenStringExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Instant;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    documents: Vec<Input>,
    #[serde(default = "default_samples")]
    samples: usize,
}

fn default_samples() -> usize {
    3
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    id: String,
    kind: String,
    source: String,
    #[serde(default)]
    dictionary: String,
    #[serde(default)]
    ignore_capitalized_names: bool,
    #[serde(default)]
    language: Option<String>,
}

/// Maps each projected Unicode scalar to its original scalar, or null for an
/// inserted separator/opaque placeholder. Protected placeholders are never edits.
#[derive(Default)]
struct Projection {
    text: String,
    source_map: Vec<Option<usize>>,
    protected: Vec<usize>,
    opaque_ranges: Vec<Value>,
    omitted_syntax: Vec<Value>,
}

impl Projection {
    fn push(&mut self, ch: char, source: Option<usize>, protected: bool) {
        if protected {
            self.protected.push(self.source_map.len());
        }
        self.text.push(ch);
        self.source_map.push(source);
    }

    fn separator(&mut self, paragraph: bool) {
        if self.text.is_empty() {
            return;
        }
        let target = if paragraph { '\n' } else { ' ' };
        if paragraph {
            if self.text.ends_with(' ') {
                self.text.pop();
                self.source_map.pop();
            }
            let trailing = self.text.chars().rev().take_while(|&ch| ch == '\n').count();
            for _ in trailing..2 {
                self.push(target, None, false);
            }
        } else if !self.text.ends_with(char::is_whitespace) {
            self.push(target, None, false);
        }
    }

    fn trim_end(&mut self) {
        while self.text.ends_with(char::is_whitespace) {
            self.text.pop();
            self.source_map.pop();
        }
    }

    fn json(mut self) -> Value {
        self.trim_end();
        json!({"text": self.text, "source_map": self.source_map,
            "protected_scalars": self.protected,
            "protected_ranges": self.protected.iter().map(|&index| [index, index + 1]).collect::<Vec<_>>(),
            "opaque_source_ranges": self.opaque_ranges, "omitted_syntax": self.omitted_syntax})
    }
}

fn syntax_wrapper(source: &[char], token: &Token) -> bool {
    if matches!(token.kind, TokenKind::HeadingStart) {
        return true;
    }
    if !token.kind.is_unlintable()
        || !source
            .get(token.span.end)
            .is_some_and(|ch| matches!(ch, '(' | '['))
    {
        return false;
    }
    // These admitted prose calls are syntax wrappers, not equation objects.
    // All actual tokens remain exported, so this ML-only adaptation is visible.
    matches!(
        token.span.get_content_string(source).as_str(),
        "text"
            | "emph"
            | "strong"
            | "underline"
            | "overline"
            | "strike"
            | "highlight"
            | "smallcaps"
            | "upper"
            | "lower"
            | "box"
            | "block"
            | "align"
            | "pad"
    )
}

fn project(source: &[char], tokens: &[Token], base: usize, opaque: bool) -> Value {
    let mut result = Projection::default();
    let mut previous_opaque = false;
    for token in tokens {
        if syntax_wrapper(source, token) {
            result
                .omitted_syntax
                .push(json!({"source_start": base + token.span.start,
                "source_end": base + token.span.end}));
            continue;
        }
        // These token kinds are semantic objects to Harper, not editable prose.
        // Passing literal URLs/addresses to an ML tagger would otherwise permit
        // a spelling edit to change a link or email destination.
        let hidden = matches!(
            token.kind,
            TokenKind::Unlintable
                | TokenKind::Regexish
                | TokenKind::Url
                | TokenKind::EmailAddress
                | TokenKind::Hostname
        );
        if hidden {
            if opaque && !previous_opaque && !token.span.is_empty() {
                result.separator(false);
                let at = result.source_map.len();
                result.push('x', None, true);
                result.opaque_ranges.push(json!({"start": at, "end": at + 1,
                    "source_start": base + token.span.start, "source_end": base + token.span.end}));
            } else if !opaque {
                result.separator(false);
            } else if let Some(range) = result.opaque_ranges.last_mut() {
                range["source_end"] = json!(base + token.span.end);
            }
            previous_opaque = true;
            continue;
        }
        if token.kind.is_paragraph_break() {
            result.separator(true);
        } else if token.kind.is_whitespace() {
            result.separator(false);
        } else {
            if previous_opaque && opaque && !token.kind.is_punctuation() {
                result.separator(false);
            }
            for (relative, &ch) in source[token.span.start..token.span.end].iter().enumerate() {
                let offset = token.span.start + relative;
                if ch.is_whitespace() {
                    result.separator(false);
                } else {
                    result.push(ch, Some(base + offset), false);
                }
            }
        }
        previous_opaque = false;
    }
    result.json()
}

fn timing<T>(work: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let value = std::hint::black_box(work());
    (value, start.elapsed().as_secs_f64() * 1_000.0)
}

fn diagnostic_json(value: &Diagnostic) -> Value {
    json!({"provider": value.provider, "message": value.message,
        "line": value.line(), "column": value.location.map(|location| location.column)})
}

fn lint_json(
    lint: &NamedLint,
    source: &[char],
    base: usize,
    tokens: &[Token],
    ignore_names: bool,
) -> Value {
    let mut absolute = lint.clone();
    absolute.span.push_by(base);
    let visible = !diagnostics(
        source,
        tokens,
        vec![absolute.clone()],
        Path::new("probe"),
        false,
        ignore_names,
    )
    .is_empty();
    let suggestions: Vec<_> = lint
        .suggestions
        .iter()
        .map(|suggestion| match suggestion {
            harper_core::linting::Suggestion::ReplaceWith(chars) => json!({
            "kind": "replace", "text": chars.iter().collect::<String>()}),
            harper_core::linting::Suggestion::InsertAfter(chars) => json!({
            "kind": "insert_after", "text": chars.iter().collect::<String>()}),
            harper_core::linting::Suggestion::Remove => json!({"kind": "remove", "text": ""}),
        })
        .collect();
    json!({"start": absolute.span.start, "end": absolute.span.end,
        "text": absolute.span.get_content_string(source), "kind": lint.lint_kind,
        "message": lint.message, "priority": lint.priority, "suggestions": suggestions,
        "shown_in_app": visible})
}

fn spell_only(dictionary: Arc<MergedDictionary>, dialect: Dialect) -> LintGroup {
    let mut group = LintGroup::new_curated(dictionary, dialect);
    group.set_all_rules_to(Some(false));
    group.config.set_rule_enabled("SpellCheck", true);
    group
}

fn export(input: &Input, samples: usize) -> Value {
    let kind = match input.kind.as_str() {
        "typst" => DocumentKind::Typst,
        "tex" => DocumentKind::Tex,
        _ => panic!("unsupported probe kind: {}", input.kind),
    };
    let language = match input.language.as_deref().unwrap_or("auto") {
        "auto" => crate::settings::WritingLanguage::Auto,
        "british" => crate::settings::WritingLanguage::British,
        "american" => crate::settings::WritingLanguage::American,
        value => panic!("unsupported probe language: {value}"),
    };
    assert!(
        input.source.len() <= 1024 * 1024,
        "probe source exceeds 1 MiB"
    );
    let options = Options {
        dictionary: &input.dictionary,
        ignore_capitalized_names: input.ignore_capitalized_names,
        ..Options::default()
    };
    let path = std::path::PathBuf::from(format!("{}.{}", input.id, input.kind));
    let check = |checker: &mut Checker| {
        checker.check_with_options(&input.source, kind, &path, true, false, language, options)
    };
    let mut checker = Checker::default();
    let (actual, first_ms) = timing(|| check(&mut checker));
    let first_stats = checker.cache_stats();
    let mut cold_ms = Vec::new();
    let mut hit_ms = Vec::new();
    for _ in 0..samples {
        let (repeat, elapsed) = timing(|| check(&mut checker));
        assert_eq!(
            repeat, actual,
            "cache-hit diagnostic mismatch: {}",
            input.id
        );
        hit_ms.push(elapsed);
        let (cold, elapsed) = timing(|| check(&mut Checker::default()));
        assert_eq!(cold, actual, "cold diagnostic mismatch: {}", input.id);
        cold_ms.push(elapsed);
    }
    let source: Vec<_> = input.source.chars().collect();
    let mut tokens = Prose(kind).parse(&source);
    let mut blocks = Vec::new();
    let mut documents = Vec::new();
    let mut bases = Vec::new();
    let mut all_lints = Vec::new();
    if let Some((_, dialect)) = checker.context {
        let dictionary = checker.dictionary.as_ref().unwrap();
        spelling::merge_dictionary_compounds(
            &source,
            &mut tokens,
            dictionary.as_ref(),
            checker.max_personal_compound_scalars,
        );
        for (token_range, range) in paragraph_ranges(&tokens, source.len()) {
            let mut relative = tokens[token_range].to_vec();
            for token in &mut relative {
                token.span.pull_by(range.start);
            }
            let text = &source[range.clone()];
            let block = checker
                .blocks
                .values()
                .find(|block| block.text.as_ref() == text && block.tokens == relative)
                .expect("probe paragraph was evicted; use a smaller fixture");
            let document = Document::new_from_chars(
                text.to_vec().into(),
                &Replay(&block.tokens),
                dictionary.as_ref(),
            );
            let lints: Vec<_> = block
                .lints
                .iter()
                .map(|lint| {
                    lint_json(
                        lint,
                        &source,
                        range.start,
                        &tokens,
                        input.ignore_capitalized_names,
                    )
                })
                .collect();
            let sentences: Vec<_> = document.get_tokens().iter_sentences().map(|sentence| json!({
                "source_start": sentence.iter().map(|token| token.span.start).min().unwrap_or(0) + range.start,
                "source_end": sentence.iter().map(|token| token.span.end).max().unwrap_or(0) + range.start,
                "opaque": project(text, sentence, range.start, true),
                "prose": project(text, sentence, range.start, false),
            })).collect();
            blocks.push(json!({"source_start": range.start, "source_end": range.end,
                "source": text.iter().collect::<String>(),
                "parser_tokens": block.tokens, "document_tokens": document.get_tokens(),
                "opaque": project(text, document.get_tokens(), range.start, true),
                "prose": project(text, document.get_tokens(), range.start, false),
                "sentences": sentences, "harper_lints": lints}));
            all_lints.extend(block.lints.iter().map(|lint| {
                let mut lint = lint.clone();
                lint.span.push_by(range.start);
                lint
            }));
            documents.push(document);
            bases.push(range.start);
        }
        let reproduced = diagnostics(
            &source,
            &tokens,
            all_lints,
            &path,
            false,
            input.ignore_capitalized_names,
        );
        assert_eq!(
            reproduced, actual,
            "export must exactly reproduce app diagnostics"
        );
        let mut spell_cold_ms = Vec::new();
        let mut spell_warm_ms = Vec::new();
        let mut spell_checker = spell_only(Arc::clone(dictionary), dialect);
        let spell = |checker: &mut LintGroup| {
            documents
                .iter()
                .zip(&bases)
                .flat_map(|(document, &base)| {
                    named_lints(checker, document)
                        .into_iter()
                        .map(move |mut lint| {
                            lint.span.push_by(base);
                            lint
                        })
                })
                .collect::<Vec<_>>()
        };
        let (spell_lints, spell_first_ms) = timing(|| spell(&mut spell_checker));
        for _ in 0..samples {
            let (repeat, elapsed) = timing(|| spell(&mut spell_checker));
            assert_eq!(repeat, spell_lints);
            spell_warm_ms.push(elapsed);
            let (cold, elapsed) =
                timing(|| spell(&mut spell_only(Arc::clone(dictionary), dialect)));
            assert_eq!(cold, spell_lints);
            spell_cold_ms.push(elapsed);
        }
        return json!({"id": input.id, "kind": input.kind,
            "source": input.source, "scalar_count": source.len(),
            "dialect": format!("{dialect:?}"), "blocks": blocks,
            "harper_diagnostics": actual.iter().map(diagnostic_json).collect::<Vec<_>>(),
            "harper_spell_only_lints": spell_lints.iter().map(|lint| lint_json(lint,
                &source, 0, &tokens, input.ignore_capitalized_names)).collect::<Vec<_>>(),
            "timings": {"production_first_check_ms": first_ms,
                "production_first_cache_hits_misses": first_stats,
                "production_cold_cache_ms": cold_ms, "production_cache_hit_ms": hit_ms,
                "spell_only_first_ms": spell_first_ms,
                "spell_only_cold_suggestion_cache_ms": spell_cold_ms,
                "spell_only_warm_suggestion_cache_ms": spell_warm_ms}});
    }
    json!({"id": input.id, "kind": input.kind, "source": input.source,
        "dialect": null, "blocks": [], "harper_diagnostics": [],
        "disabled_reason": "writing language is not English"})
}

#[test]
#[ignore = "opt-in production Harper export; set TIPTOPTYP_GRAMMAR_PROBE_INPUT/OUTPUT and use --release"]
fn export_production_harper_fragments() {
    assert!(
        !std::hint::black_box(cfg!(debug_assertions)),
        "use --release"
    );
    let input =
        std::env::var_os("TIPTOPTYP_GRAMMAR_PROBE_INPUT").expect("set probe input JSON path");
    let output =
        std::env::var_os("TIPTOPTYP_GRAMMAR_PROBE_OUTPUT").expect("set probe output JSON path");
    let input: Request = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    assert!((1..=20).contains(&input.samples));
    assert!(input.documents.len() <= 4096);
    let documents: Vec<_> = input
        .documents
        .iter()
        .map(|document| export(document, input.samples))
        .collect();
    let output_value = json!({"schema": 1, "harper_version": "2.11.0", "profile": "release",
        "os": std::env::consts::OS, "architecture": std::env::consts::ARCH,
        "coordinate_unit": "Unicode scalar, zero-based, half-open",
        "provenance": "Production Checker::check_with_options; cached paragraphs replayed with the production dictionary and Harper Document normalization. Export asserts exact diagnostic equality.",
        "projection_contract": "Harper consumes source plus masked tokens, not either projected string. Prose omits unlintable spans; opaque replaces them with protected x. Both collapse whitespace. source_map maps output scalars to original source; null means inserted separator/placeholder. These projections are ML adapters, not alternate Harper parsers.",
        "timing_contract": "Production includes syntax parsing, cache lookup/linting and diagnostic mapping. Cold cache uses a fresh Checker with process-resident dictionary. First process document may include dictionary initialization. Spell-only configures the curated group with only SpellCheck enabled; uses the identical already-built Documents and dictionary; excludes Document parsing/construction/diagnostic filtering (cold suggestion-cache samples include constructing the configured LintGroup), so it is not an end-to-end latency comparison. Export and serialization are outside timings.",
        "samples": input.samples, "documents": documents});
    std::fs::write(output, serde_json::to_vec_pretty(&output_value).unwrap()).unwrap();
}

#[test]
fn projection_keeps_unicode_offsets_and_protects_opaque_math() {
    let source: Vec<_> = "é before $x$, after.".chars().collect();
    let tokens = Prose(DocumentKind::Typst).parse(&source);
    let projection = project(&source, &tokens, 10, true);
    let text = projection["text"].as_str().unwrap();
    assert_eq!(text, "é before x, after.");
    assert_eq!(projection["source_map"][0], 10);
    let index = text.chars().position(|ch| ch == 'x').unwrap();
    assert!(projection["source_map"][index].is_null());
    assert_eq!(projection["protected_scalars"], json!([index]));
    let prose = project(&source, &tokens, 0, false);
    assert!(!prose["text"].as_str().unwrap().contains('$'));
    assert_eq!(
        projection["source_map"].as_array().unwrap().len(),
        text.chars().count()
    );
}

#[test]
fn projection_protects_urls_email_addresses_and_hostnames() {
    use harper_core::Span;

    for (kind, payload) in [
        (TokenKind::Url, "https://example.com/tyop"),
        (TokenKind::EmailAddress, "typo@example.com"),
        (TokenKind::Hostname, "typo.example.org"),
    ] {
        let source: Vec<_> = format!("Before {payload} afterward").chars().collect();
        let start = 7;
        let end = start + payload.chars().count();
        let tokens = vec![
            Token::new(Span::new(0, 6), TokenKind::Word(None)),
            Token::new(Span::new(6, start), TokenKind::Space(1)),
            Token::new(Span::new(start, end), kind),
            Token::new(Span::new(end, end + 1), TokenKind::Space(1)),
            Token::new(Span::new(end + 1, source.len()), TokenKind::Word(None)),
        ];
        let opaque = project(&source, &tokens, 0, true);
        assert_eq!(opaque["text"], "Before x afterward");
        assert_eq!(opaque["protected_scalars"], json!([start]));
        assert!(opaque["source_map"][start].is_null());
        assert_eq!(opaque["opaque_source_ranges"][0]["source_start"], start);
        assert_eq!(opaque["opaque_source_ranges"][0]["source_end"], end);
        let prose = project(&source, &tokens, 0, false);
        assert_eq!(prose["text"], "Before afterward");
        assert!(prose["protected_scalars"].as_array().unwrap().is_empty());
    }
}

#[test]
fn exported_cache_inputs_reproduce_filtered_app_diagnostics() {
    for kind in ["tex", "typst"] {
        let source = if kind == "tex" {
            "  This is the the result.\\[x=1,\\]\nwhich is misspelld.\n\nBarbato proves it."
        } else {
            "  This is the the result. $ x=1, $\nwhich is misspelld.\n\nBarbato proves it."
        };
        let report = export(
            &Input {
                id: "regression".into(),
                kind: kind.into(),
                source: source.into(),
                dictionary: String::new(),
                ignore_capitalized_names: true,
                language: None,
            },
            1,
        );
        assert!(!report["harper_diagnostics"].as_array().unwrap().is_empty());
        assert!(report["blocks"].as_array().unwrap().len() >= 2);
        let spelling = report["harper_spell_only_lints"].as_array().unwrap();
        assert!(spelling.iter().any(|lint| lint["text"] == "misspelld"));
        assert!(
            !spelling
                .iter()
                .any(|lint| lint["text"] == "Barbato" && lint["shown_in_app"] == true)
        );
    }
}
