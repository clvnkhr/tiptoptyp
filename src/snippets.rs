//! User snippets use the same validated completion transaction as LSP snippets.
use crate::{document::DocumentKind, tinymist::CompletionItem};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Language {
    Typst,
    Tex,
    Both,
}
impl Language {
    fn accepts(self, kind: DocumentKind) -> bool {
        matches!(
            (self, kind),
            (Self::Both, DocumentKind::Typst | DocumentKind::Tex)
                | (Self::Typst, DocumentKind::Typst)
                | (Self::Tex, DocumentKind::Tex)
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snippet {
    pub prefix: String,
    pub description: String,
    pub language: Language,
    pub body: String,
}
/// Convert source to the wire snippet format without interpreting ordinary
/// math dollars or TeX backslashes. Only explicit numbered ${...} fields are
/// reserved in custom source; bare $1 and $0 remain ordinary source text.
pub(crate) fn source_to_lsp(source: &str) -> Result<String, String> {
    fn encode(chars: &[char], output: &mut String) -> Result<(), String> {
        let mut index = 0;
        while index < chars.len() {
            if chars[index] == '$'
                && chars.get(index + 1) == Some(&'{')
                && chars.get(index + 2).is_some_and(char::is_ascii_digit)
                && chars[..index]
                    .iter()
                    .rev()
                    .take_while(|c| **c == '\\')
                    .count()
                    .is_multiple_of(2)
            {
                let number_end = index
                    + 2
                    + chars[index + 2..]
                        .iter()
                        .take_while(|c| c.is_ascii_digit())
                        .count();
                let number: String = chars[index + 2..number_end].iter().collect();
                number.parse::<u32>().map_err(|_| {
                    "Field numbers must fit in a 32-bit unsigned integer".to_owned()
                })?;
                output.push_str("${");
                output.push_str(&number);
                match chars.get(number_end) {
                    Some('}') => { index = number_end + 1; }
                    Some(':') => {
                        let start = number_end + 1;
                        let mut depth = 0;
                        let mut close = start;
                        while close < chars.len() {
                            match chars[close] {
                                '\\' if chars.get(close + 1).is_some_and(|c| matches!(c, '{' | '}' | '\\')) => { close += 2; continue; }
                                '{' => depth += 1,
                                '}' if depth == 0 => break,
                                '}' => depth -= 1,
                                _ => {}
                            }
                            close += 1;
                        }
                        if close == chars.len() { return Err("Close the snippet field with }: ${1:text}".into()); }
                        output.push(':');
                        encode(&chars[start..close], output)?;
                        index = close + 1;
                    }
                    _ => return Err("Use ${1:text} for a field, ${1} to repeat it, or ${0} for the final cursor".into()),
                }
                output.push('}');
            } else {
                if matches!(chars[index], '$' | '}' | '\\') {
                    output.push('\\');
                }
                output.push(chars[index]);
                index += 1;
            }
        }
        Ok(())
    }
    let mut output = String::with_capacity(source.len());
    encode(&source.chars().collect::<Vec<_>>(), &mut output)?;
    Ok(output)
}

pub(crate) fn validate(snippets: &[Snippet]) -> Result<(), String> {
    for (index, snippet) in snippets.iter().enumerate() {
        let label = if snippet.prefix.is_empty() {
            format!("Snippet {}", index + 1)
        } else {
            format!("Snippet “{}”", snippet.prefix)
        };
        if snippet.prefix.is_empty()
            || !snippet
                .prefix
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
        {
            return Err(format!(
                "{label}: enter a prefix using letters, numbers, hyphens or underscores"
            ));
        }
        let encoded = source_to_lsp(&snippet.body).map_err(|error| format!("{label}: {error}"))?;
        crate::completion_edit::expand_lsp_snippet(&encoded)
            .map_err(|error| format!("{label}: {error}"))?;
    }
    Ok(())
}
pub(crate) fn items(
    snippets: &[Snippet],
    kind: DocumentKind,
    source: &str,
    cursor: usize,
) -> Option<Vec<CompletionItem>> {
    if snippets.is_empty() {
        return None;
    }
    let byte = source
        .char_indices()
        .nth(cursor)
        .map_or(source.len(), |(byte, _)| byte);
    let query = source[..byte]
        .rsplit(|c: char| !c.is_alphanumeric() && !matches!(c, '_' | '-'))
        .next()
        .unwrap_or_default();
    if query.is_empty() {
        return None;
    }
    let items: Vec<_> = snippets
        .iter()
        .filter(|s| s.language.accepts(kind) && s.prefix.starts_with(query))
        .filter_map(|s| {
            Some(CompletionItem {
                origin: crate::lsp::protocol::CompletionOrigin::Snippet,
                label: s.prefix.clone(),
                detail: Some(s.description.clone()),
                documentation: Some(s.body.clone()),
                filter_text: Some(s.prefix.clone()),
                sort_text: None,
                insert_text: source_to_lsp(&s.body).ok()?,
                insert_text_is_snippet: true,
                text_edit: None,
                additional_text_edits: Vec::new(),
            })
        })
        .collect();
    (!items.is_empty()).then_some(items)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_source_dollars_backslashes_quotes_and_newlines_are_literal() {
        for source in [
            r"$x$ and $1$ and $0$",
            r"$alpha$",
            r"\begin{align} x & y \\ \end{align}",
            r"\$ and \{x\}",
            r"\${1} and \${1:literal}",
            "#let name = \"α\"\n$ x $",
            "${USER} ${word:default}",
        ] {
            let encoded = source_to_lsp(source).unwrap();
            let result = crate::completion_edit::expand_lsp_snippet(&encoded).unwrap();
            assert_eq!(result.text, source);
            assert!(result.fields.is_empty());
        }
    }

    #[test]
    fn explicit_fields_preserve_tex_defaults_and_mirrors() {
        let source = r"${1:\frac{α}{β}} + ${1}; ${2:$x$ \\} ${0}";
        let result =
            crate::completion_edit::expand_lsp_snippet(&source_to_lsp(source).unwrap()).unwrap();
        assert_eq!(result.text, r"\frac{α}{β} + \frac{α}{β}; $x$ \\ ");
        assert_eq!(result.fields.iter().filter(|(id, _)| *id == 1).count(), 2);
        assert_eq!(result.cursor, 0);
        assert!(source_to_lsp("${1:unfinished").is_err());
        assert!(source_to_lsp("${999999999999999999999999}").is_err());
    }

    #[test]
    fn snippets_are_scoped_and_expand_through_the_completion_transaction() {
        let snippets = vec![Snippet {
            prefix: "greet".into(),
            description: "Greeting".into(),
            language: Language::Typst,
            body: "Hello ${1:world}!${0}".into(),
        }];
        validate(&snippets).unwrap();
        assert!(items(&snippets, DocumentKind::Tex, "gre", 3).is_none());
        assert!(items(&snippets, DocumentKind::Typst, " ", 1).is_none());
        let item = items(&snippets, DocumentKind::Typst, "gre", 3)
            .unwrap()
            .remove(0);
        let result =
            crate::completion_edit::prepare_completion_application("gre", 3, &item).unwrap();
        assert_eq!(result.source, "Hello world!");
        assert_eq!(result.cursor, 6);
    }
}

/// A short-lived linked-field edit. All mirrors are applied inside the same
/// document transaction as the user's keystroke; no background writer exists.
pub(crate) struct Session {
    pub(crate) key: tiptoptyp_core::document::DocumentKey,
    before: String,
    fields: Vec<(u32, std::ops::Range<usize>)>,
    active: u32,
}
impl Session {
    pub(crate) fn new(
        key: tiptoptyp_core::document::DocumentKey,
        source: &str,
        fields: Vec<(u32, std::ops::Range<usize>)>,
    ) -> Option<Self> {
        // Nested placeholders still insert correctly, but cannot safely retain
        // independent linked selections after their enclosing field is replaced.
        if fields.iter().enumerate().any(|(index, (_, a))| {
            fields[index + 1..].iter().any(|(_, b)| {
                (a.start < b.start && b.start < a.end)
                    || (b.start < a.start && a.start < b.end)
                    || (a.start == b.start && (!a.is_empty() || !b.is_empty()))
            })
        }) {
            return None;
        }
        let active = fields
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| *id != 0)
            .min()?;
        Some(Self {
            key,
            before: source.into(),
            fields,
            active,
        })
    }
    pub(crate) fn selection(&self) -> std::ops::Range<usize> {
        self.fields
            .iter()
            .find(|(id, _)| *id == self.active)
            .unwrap()
            .1
            .clone()
    }
    pub(crate) fn advance(&mut self, backwards: bool) -> Option<std::ops::Range<usize>> {
        let next = if backwards {
            self.fields
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| *id != 0 && *id < self.active)
                .max()
        } else {
            self.fields
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| *id > self.active)
                .min()
                .or(Some(0))
        }?;
        let range = self
            .fields
            .iter()
            .find(|(id, _)| *id == next)
            .map(|(_, range)| range.clone());
        self.active = next;
        range
    }
    pub(crate) fn finished(&self) -> bool {
        self.active == 0
    }
    pub(crate) fn update(&mut self, source: &mut String, caret: usize) -> Option<usize> {
        let old: Vec<char> = self.before.chars().collect();
        let new: Vec<char> = source.chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let primary = self.selection();
        let end = old.len() - suffix;
        if prefix < primary.start || end > primary.end || caret < primary.start {
            return None;
        }
        let delta = new.len() as isize - old.len() as isize;
        let new_end = primary.end.checked_add_signed(delta)?;
        let value: Vec<char> = new[primary.start..new_end].to_vec();
        let mut edits: Vec<_> = self
            .fields
            .iter()
            .filter(|(id, _)| *id == self.active)
            .map(|(_, range)| range.clone())
            .collect();
        edits.sort_by_key(|range| range.start);
        // Nested or coincident fields cannot be mirrored safely.
        if edits.windows(2).any(|pair| pair[0].end >= pair[1].start) {
            return None;
        }
        let mut result = old;
        for range in edits.iter().rev() {
            result.splice(range.clone(), value.iter().copied());
        }
        let shift = |position: usize| -> Option<usize> {
            let adjustment: isize = edits
                .iter()
                .filter(|range| range.end <= position && range.start < position)
                .map(|range| value.len() as isize - range.len() as isize)
                .sum();
            position.checked_add_signed(adjustment)
        };
        let start = shift(primary.start)?;
        for (id, range) in &mut self.fields {
            let mapped = shift(range.start)?;
            let end = if *id == self.active {
                mapped + value.len()
            } else {
                shift(range.end)?
            };
            *range = mapped..end;
        }
        *source = result.into_iter().collect();
        self.before.clone_from(source);
        Some(start + caret - primary.start)
    }
}

#[cfg(test)]
mod linked_tests {
    use super::*;
    #[test]
    fn linked_names_follow_typing_unicode_deletion_and_finish() {
        let mut source = "\\begin{}\n\\end{}".to_string();
        let mut session = Session::new(
            tiptoptyp_core::document::DocumentKey::new(
                tiptoptyp_core::document::WindowSessionId::new(1),
                1,
                1,
            ),
            &source,
            vec![(1, 7..7), (1, 14..14), (0, 15..15)],
        )
        .unwrap();
        source.insert_str(7, "enumerate");
        assert_eq!(session.update(&mut source, 16), Some(16));
        assert_eq!(source, "\\begin{enumerate}\n\\end{enumerate}");
        assert_eq!(session.selection(), 7..16);
        source.replace_range(7..16, "α");
        assert_eq!(session.update(&mut source, 8), Some(8));
        assert_eq!(source, "\\begin{α}\n\\end{α}");
        assert!(session.advance(false).is_some());
        assert!(session.finished());
    }
    #[test]
    fn edits_outside_field_end_the_session_without_overwriting_text() {
        let mut source = "a name name".to_string();
        let mut session = Session::new(
            tiptoptyp_core::document::DocumentKey::new(
                tiptoptyp_core::document::WindowSessionId::new(1),
                1,
                1,
            ),
            &source,
            vec![(1, 2..6), (1, 7..11)],
        )
        .unwrap();
        source.insert(0, 'x');
        assert_eq!(session.update(&mut source, 1), None);
        assert_eq!(source, "xa name name");
    }
}
