//! Typst syntax is admitted by our parser before it reaches a prose checker.
//! The wire mirror preserves line and UTF-16 coordinates, never source edits.
pub(super) struct TypstProse {
    pub(super) text: String,
    excluded_prefix: Vec<usize>,
}

impl TypstProse {
    pub(super) fn new(source: &str) -> Self {
        let characters: Vec<_> = source.chars().collect();
        let tokens = crate::writing::ltex_prose_tokens(&characters);
        let mut allowed = vec![false; characters.len()];
        let mut equation = vec![false; characters.len()];
        for token in tokens {
            if token.kind.is_unlintable()
                && let Some(span) = characters.get(token.span.start..token.span.end)
                && span.first() == Some(&'$')
            {
                equation[token.span.start] = true;
            }
            if !token.kind.is_unlintable()
                && !matches!(
                    token.kind,
                    harper_core::TokenKind::Regexish
                        | harper_core::TokenKind::Url
                        | harper_core::TokenKind::EmailAddress
                        | harper_core::TokenKind::Hostname
                )
                && let Some(span) = allowed.get_mut(token.span.start..token.span.end)
            {
                span.fill(true);
            }
        }
        // Typst's dash syntax may be omitted by the prose tokenizer. Keep
        // word-internal dashes: erasing them splits eponyms into misspellings.
        let mut cursor = 0;
        while cursor < characters.len() {
            if !matches!(characters[cursor], '-' | '–' | '—') {
                cursor += 1;
                continue;
            }
            let start = cursor;
            while cursor < characters.len() && matches!(characters[cursor], '-' | '–' | '—') {
                cursor += 1;
            }
            if start > 0
                && cursor < characters.len()
                && allowed[start - 1]
                && allowed[cursor]
                && characters[start - 1].is_alphabetic()
                && characters[cursor].is_alphabetic()
            {
                allowed[start..cursor].fill(true);
            }
        }
        let mut text = String::with_capacity(source.len());
        for (index, character) in characters.into_iter().enumerate() {
            if equation[index] {
                // Keep an opaque noun in the sentence, so removing an inline
                // equation does not invent an agreement error in nearby prose.
                text.push('x');
            } else if allowed[index] || matches!(character, '\r' | '\n') {
                text.push(character);
            } else {
                for _ in 0..character.len_utf16() {
                    text.push(' ');
                }
            }
        }
        let mut excluded_prefix = Vec::with_capacity(allowed.len() + 1);
        excluded_prefix.push(0);
        for allowed in allowed {
            excluded_prefix.push(excluded_prefix.last().copied().unwrap() + usize::from(!allowed));
        }
        Self {
            text,
            excluded_prefix,
        }
    }

    pub(super) fn admits(&self, start: usize, end: usize) -> bool {
        // A server may publish thousands of wide ranges. One prefix index
        // makes admission constant-time instead of rescanning each range.
        start < end
            && self
                .excluded_prefix
                .get(start)
                .zip(self.excluded_prefix.get(end))
                .is_some_and(|(start, end)| start == end)
    }
}

/// Named mathematical compounds are proper names, rather than misspelled
/// English vocabulary. Keep ordinary lowercase compounds eligible for checks.
pub(super) fn mathematical_name(text: &str) -> bool {
    let mut parts = text.split(['-', '–', '—']).filter(|part| !part.is_empty());
    let name = |part: &str| {
        let mut chars = part.chars();
        chars.next().is_some_and(char::is_uppercase)
            && chars.clone().next().is_some()
            && chars.all(char::is_alphabetic)
    };
    parts.next().is_some_and(name) && parts.next().is_some_and(name) && parts.all(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typst_code_math_and_template_arguments_are_not_prose() {
        let source = include_str!("../../tests/fixtures/ltex/typst-mathematical-prose.typ");
        let prose = TypstProse::new(source);
        for token in [
            "center",
            "datetime",
            "nabla",
            "frac",
            "partial_t",
            "alpha",
            "Khor",
            "Xiaoyan",
        ] {
            for (byte, _) in source.match_indices(token) {
                let start = source[..byte].chars().count();
                assert!(
                    !prose.admits(start, start + token.chars().count()),
                    "{token}"
                );
            }
        }
        let byte = source.find("This are a test").unwrap();
        let start = source[..byte].chars().count();
        assert!(prose.admits(start, start + "This are a test".chars().count()));
        assert!(prose.text.contains("This are a test. We has two result."));
    }

    #[test]
    fn hidden_astral_characters_and_crlf_preserve_lsp_coordinates() {
        let source = "#let emoji = \"😀\"\r\nText #emph[with an error] and $alpha + nabla$.";
        let prose = TypstProse::new(source);
        let original: Vec<_> = source
            .split('\n')
            .map(|line| line.encode_utf16().count())
            .collect();
        let masked: Vec<_> = prose
            .text
            .split('\n')
            .map(|line| line.encode_utf16().count())
            .collect();
        assert_eq!(original, masked);
        assert!(prose.text.contains("with an error"));
        assert!(!prose.text.contains("emoji"));
        assert!(!prose.text.contains("nabla"));
    }

    #[test]
    fn rendered_string_arguments_remain_prose_but_configuration_and_authors_do_not() {
        let source = "#text(font: \"Libertinus\", \"This are a test.\")\n#heading(\"We has a result.\")\n#figure(rect(), caption: \"An useful result.\")\n#subtitle((\"Calvin Khor\",), datetime.today())";
        let prose = TypstProse::new(source);
        for rendered in ["This are a test.", "We has a result.", "An useful result."] {
            let byte = source.find(rendered).unwrap();
            let start = source[..byte].chars().count();
            assert!(
                prose.admits(start, start + rendered.chars().count()),
                "{rendered}"
            );
            assert!(prose.text.contains(rendered));
        }
        for code in ["Libertinus", "Calvin Khor", "datetime"] {
            assert!(!prose.text.contains(code));
        }
    }

    #[test]
    fn equations_keep_their_grammatical_placeholder_when_punctuation_is_exposed() {
        let source = "Let $alpha,$ be positive.\n$ beta. $";
        let prose = TypstProse::new(source);
        assert_eq!(prose.text.matches('x').count(), 2);
        assert!(prose.text.contains(','));
        assert_eq!(prose.text.matches('.').count(), 2);
        assert_eq!(
            source.encode_utf16().count(),
            prose.text.encode_utf16().count()
        );
    }

    #[test]
    fn word_internal_dashes_preserve_eponyms_and_ordinary_spelling_checks() {
        let source = "Leray--Hopf and well-posedniss beside $alpha-beta$.";
        let prose = TypstProse::new(source);
        assert!(prose.text.contains("Leray--Hopf"));
        assert!(prose.text.contains("well-posedniss"));
        assert!(!prose.text.contains("alpha-beta"));
        assert!(prose.admits(0, "Leray--Hopf".chars().count()));
    }

    #[test]
    fn only_capitalized_eponym_compounds_are_exempt_from_spelling() {
        for name in ["Leray--Hopf", "Navier–Stokes", "Cauchy-Schwarz"] {
            assert!(mathematical_name(name));
        }
        for word in [
            "well-posedniss",
            "Leray",
            "misspelt--word",
            "Leray--hopff",
            "Leray--123",
        ] {
            assert!(!mathematical_name(word));
        }
    }
}
