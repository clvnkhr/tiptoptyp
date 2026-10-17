//! User snippets use the same validated completion transaction as LSP snippets.
use crate::{document::DocumentKind, tinymist::CompletionItem};
use serde::{Deserialize, Serialize};
use tiptoptyp_core::text::{LspRange, LspTextEdit, ScalarOffset, lsp_position_at_scalar};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Scope {
    #[default]
    Anywhere,
    Text,
    Math,
    Code,
}

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
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_placeholder_open")]
    pub placeholder_open: String,
    #[serde(default = "default_placeholder_close")]
    pub placeholder_close: String,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub line_start: bool,
}
fn default_enabled() -> bool {
    true
}

pub(crate) fn default_placeholder_open() -> String {
    "{{".into()
}

pub(crate) fn default_placeholder_close() -> String {
    "}}".into()
}

impl Snippet {
    pub(crate) fn source_to_lsp(&self) -> Result<String, String> {
        let encoded = source_to_lsp(&self.body, &self.placeholder_open, &self.placeholder_close)?;
        // Mirrored nested defaults can expand exponentially. Reject them before
        // any caller can preview, insert or validate their LSP expansion.
        if nested_numbered_fields(&encoded) {
            return Err(NESTED_FIELDS_ERROR.into());
        }
        Ok(encoded)
    }
}

#[derive(Debug)]
struct SourceField {
    id: u32,
    start: usize,
    number: std::ops::Range<usize>,
    end: usize,
    default: Option<(std::ops::Range<usize>, Vec<SourceField>)>,
}

const MAX_FIELD_DEPTH: usize = 32;
const MAX_FIELDS: usize = 4096;
const NESTED_FIELDS_ERROR: &str = "keep numbered fields separate. Nested or overlapping fields cannot be linked; ordinary TeX and Typst braces are allowed.";

pub(crate) fn validate_placeholder_delimiters(open: &str, close: &str) -> Result<(), String> {
    for (label, delimiter) in [("Opening", open), ("Closing", close)] {
        let length = delimiter.chars().count();
        if !(1..=16).contains(&length)
            || delimiter.chars().any(|c| {
                c.is_alphanumeric()
                    || c.is_whitespace()
                    || c.is_control()
                    || matches!(c, ':' | '\\')
            })
        {
            return Err(format!(
                "{label} placeholder marker: use 1–16 punctuation or symbol characters, without spaces, letters, numbers, : or \\",
            ));
        }
    }
    if open.starts_with(close) || close.starts_with(open) {
        return Err(
            "Placeholder markers must be different; neither may start with the other".into(),
        );
    }
    Ok(())
}

struct FieldParser<'a> {
    source: &'a [char],
    open: Vec<char>,
    close: Vec<char>,
    count: usize,
}

impl FieldParser<'_> {
    fn error(&self, at: usize, message: &str) -> String {
        let line = self.source[..at].iter().filter(|c| **c == '\n').count() + 1;
        let column = self.source[..at]
            .iter()
            .rev()
            .take_while(|c| **c != '\n')
            .count()
            + 1;
        format!("{message} (line {line}, column {column})")
    }

    fn opens_at(&self, at: usize) -> bool {
        self.source
            .get(at..)
            .is_some_and(|tail| tail.starts_with(&self.open))
    }

    fn closes_at(&self, at: usize) -> bool {
        self.source
            .get(at..)
            .is_some_and(|tail| tail.starts_with(&self.close))
    }

    fn literal_before_overlapping_open(&self, at: usize) -> bool {
        // A normal language brace may directly enclose a placeholder:
        // `\text{{{1:hello}}}` has one literal brace before the `{{` marker.
        !self
            .source
            .get(at + self.open.len())
            .is_some_and(char::is_ascii_digit)
            && self.opens_at(at + 1)
    }

    fn field(&mut self, start: usize, depth: usize) -> Result<SourceField, String> {
        if depth > MAX_FIELD_DEPTH {
            return Err(self.error(start, "Snippet fields are nested too deeply"));
        }
        self.count += 1;
        if self.count > MAX_FIELDS {
            return Err(self.error(start, "A snippet can contain at most 4096 fields"));
        }
        let number_start = start + self.open.len();
        let mut after_number = number_start;
        while self
            .source
            .get(after_number)
            .is_some_and(char::is_ascii_digit)
        {
            after_number += 1;
        }
        if number_start == after_number {
            return Err(self.error(
                number_start,
                "A placeholder number must be 0 or a positive integer",
            ));
        }
        let number = number_start..after_number;
        let id = self.source[number.clone()]
            .iter()
            .collect::<String>()
            .parse::<u32>()
            .map_err(|_| {
                self.error(
                    number_start,
                    "Placeholder numbers must fit in a 32-bit unsigned integer",
                )
            })?;
        if self.closes_at(after_number) {
            return Ok(SourceField {
                id,
                start,
                number,
                end: after_number + self.close.len(),
                default: None,
            });
        }
        if self.source.get(after_number) != Some(&':') {
            let close: String = self.close.iter().collect();
            return Err(self.error(
                after_number,
                &format!("Put : before the default text, or close the placeholder with {close}"),
            ));
        }
        if id == 0 {
            return Err(self.error(
                start,
                "The final cursor placeholder (0) cannot have a default",
            ));
        }
        let default_start = after_number + 1;
        let mut at = default_start;
        let mut nested = Vec::new();
        // Only balance an ordinary language group when it could overlap the
        // closing marker. Other brackets, math, quotes and commands are literal.
        let pair = match self.close[0] {
            '}' => Some(('{', '}')),
            ']' => Some(('[', ']')),
            ')' => Some(('(', ')')),
            _ => None,
        };
        let mut group_depth = 0_usize;
        while at < self.source.len() {
            if self.opens_at(at) && !self.literal_before_overlapping_open(at) {
                let field = self.field(at, depth + 1)?;
                at = field.end;
                nested.push(field);
                continue;
            }
            if group_depth == 0 && self.closes_at(at) {
                return Ok(SourceField {
                    id,
                    start,
                    number,
                    end: at + self.close.len(),
                    default: Some((default_start..at, nested)),
                });
            }
            if self.source[at] == '\\' && self.source.get(at + 1) == Some(&'\\') {
                at += 2;
                continue;
            }
            if let Some((open, close)) = pair {
                // Escaped source braces are literal, including `\}` immediately
                // before a closing marker. A backslash before a placeholder is
                // still ordinary text, rather than an alternate escape grammar.
                if self.source[at] == '\\'
                    && !self.opens_at(at + 1)
                    && self
                        .source
                        .get(at + 1)
                        .is_some_and(|c| *c == open || *c == close)
                {
                    at += 2;
                    continue;
                }
                if self.source[at] == open {
                    group_depth += 1;
                } else if self.source[at] == close {
                    group_depth = group_depth.saturating_sub(1);
                }
            }
            at += 1;
        }
        let close: String = self.close.iter().collect();
        Err(self.error(start, &format!("Close the placeholder with {close}; ordinary language groups inside it must also be closed")))
    }

    fn fields(&mut self) -> Result<Vec<SourceField>, String> {
        let mut fields = Vec::new();
        let mut at = 0;
        while at < self.source.len() {
            if self.opens_at(at) && !self.literal_before_overlapping_open(at) {
                let field = self.field(at, 0)?;
                at = field.end;
                fields.push(field);
            } else {
                at += 1;
            }
        }
        Ok(fields)
    }
}

fn parse_fields(source: &[char], open: &str, close: &str) -> Result<Vec<SourceField>, String> {
    validate_placeholder_delimiters(open, close)?;
    FieldParser {
        source,
        open: open.chars().collect(),
        close: close.chars().collect(),
        count: 0,
    }
    .fields()
}

/// Convert the shared placeholder grammar to the LSP wire format. Ordinary
/// language source, including dollars, backslashes and commands, stays literal.
pub(crate) fn source_to_lsp(source: &str, open: &str, close: &str) -> Result<String, String> {
    fn literal(chars: &[char], output: &mut String) {
        for &c in chars {
            if matches!(c, '$' | '}' | '\\') {
                output.push('\\');
            }
            output.push(c);
        }
    }
    fn encode(
        chars: &[char],
        range: std::ops::Range<usize>,
        fields: &[SourceField],
        output: &mut String,
    ) {
        let mut at = range.start;
        for field in fields {
            literal(&chars[at..field.start], output);
            output.push_str("${");
            output.push_str(&field.id.to_string());
            if let Some((range, nested)) = &field.default {
                output.push(':');
                encode(chars, range.clone(), nested, output);
            }
            output.push('}');
            at = field.end;
        }
        literal(&chars[at..range.end], output);
    }
    let chars: Vec<_> = source.chars().collect();
    let fields = parse_fields(&chars, open, close)?;
    let mut output = String::with_capacity(source.len());
    encode(&chars, 0..chars.len(), &fields, &mut output);
    Ok(output)
}

/// Explicitly rewrite only recognized placeholders, preserving their defaults
/// and all ordinary source. Refuse pairs that would reinterpret literal text.
pub(crate) fn rewrite_placeholders(
    source: &str,
    old_open: &str,
    old_close: &str,
    new_open: &str,
    new_close: &str,
) -> Result<String, String> {
    fn rewrite(
        chars: &[char],
        range: std::ops::Range<usize>,
        fields: &[SourceField],
        open: &str,
        close: &str,
        output: &mut String,
    ) {
        let mut at = range.start;
        for field in fields {
            output.extend(&chars[at..field.start]);
            output.push_str(open);
            output.extend(&chars[field.number.clone()]);
            if let Some((range, nested)) = &field.default {
                output.push(':');
                rewrite(chars, range.clone(), nested, open, close, output);
            }
            output.push_str(close);
            at = field.end;
        }
        output.extend(&chars[at..range.end]);
    }
    validate_placeholder_delimiters(new_open, new_close)?;
    let chars: Vec<_> = source.chars().collect();
    let fields = parse_fields(&chars, old_open, old_close)?;
    let mut output = String::with_capacity(source.len());
    rewrite(
        &chars,
        0..chars.len(),
        &fields,
        new_open,
        new_close,
        &mut output,
    );
    if source_to_lsp(source, old_open, old_close)? != source_to_lsp(&output, new_open, new_close)? {
        return Err("Those markers conflict with literal source text. Choose another pair or keep the source unchanged".into());
    }
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
        let encoded = snippet
            .source_to_lsp()
            .map_err(|error| format!("{label}: {error}"))?;
        let expansion = crate::completion_edit::expand_lsp_snippet(&encoded)
            .map_err(|error| format!("{label}: {error}"))?;
        if fields_overlap(&expansion.fields) {
            return Err(format!("{label}: {NESTED_FIELDS_ERROR}"));
        }
        if coincident_editable_fields(&expansion.fields) {
            return Err(format!(
                "{label}: editable fields cannot share an empty position. Add text, separators or defaults between fields."
            ));
        }
    }
    Ok(())
}
fn nested_numbered_fields(encoded: &str) -> bool {
    let chars: Vec<_> = encoded.chars().collect();
    let mut depth = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == '$'
            && chars.get(i + 1) == Some(&'{')
            && chars.get(i + 2).is_some_and(char::is_ascii_digit)
        {
            if depth > 0 {
                return true;
            }
            depth += 1;
            i += 2;
        } else {
            if chars[i] == '}' {
                depth -= 1;
            }
            i += 1;
        }
    }
    false
}

fn fields_overlap(fields: &[(u32, std::ops::Range<usize>)]) -> bool {
    fields.iter().enumerate().any(|(index, (_, a))| {
        fields[index + 1..].iter().any(|(_, b)| {
            (a.start < b.start && b.start < a.end)
                || (b.start < a.start && a.start < b.end)
                || (a.start == b.start && (!a.is_empty() || !b.is_empty()))
        })
    })
}

fn coincident_editable_fields(fields: &[(u32, std::ops::Range<usize>)]) -> bool {
    fields.iter().enumerate().any(|(index, (id, a))| {
        *id != 0
            && a.is_empty()
            && fields[index + 1..]
                .iter()
                .any(|(other, b)| *other != 0 && b.is_empty() && a.start == b.start)
    })
}

/// Typed prefixes are deliberately plain words: no regex evaluation or automatic
/// source replacement happens while typing.
fn prefix(source: &str, cursor: usize) -> (usize, &str) {
    let byte = source
        .char_indices()
        .nth(cursor)
        .map_or(source.len(), |(byte, _)| byte);
    let query = source[..byte]
        .rsplit(|c: char| !c.is_alphanumeric() && !matches!(c, '_' | '-'))
        .next()
        .unwrap_or_default();
    (byte - query.len(), query)
}

#[cfg(test)]
pub(crate) fn items(
    snippets: &[Snippet],
    kind: DocumentKind,
    source: &str,
    cursor: usize,
) -> Option<Vec<CompletionItem>> {
    let (start, _) = prefix(source, cursor);
    let context = match kind {
        DocumentKind::Typst => typst_context(&typst_syntax::Source::detached(source), start),
        DocumentKind::Tex => tex_context(source, start),
        _ => None,
    };
    items_from(&[], snippets, kind, source, cursor, context)
}

pub(crate) fn items_from(
    presets: &[Snippet],
    custom: &[Snippet],
    kind: DocumentKind,
    source: &str,
    cursor: usize,
    context: Option<Scope>,
) -> Option<Vec<CompletionItem>> {
    let context = context?;
    let (start_byte, query) = prefix(source, cursor);
    if query.is_empty() {
        return None;
    }
    let eligible = |s: &&Snippet| {
        s.enabled
            && s.language.accepts(kind)
            && s.prefix.starts_with(query)
            && (s.scope == Scope::Anywhere || s.scope == context)
            && (!s.line_start || {
                let before = source[..start_byte].rsplit('\n').next().unwrap_or_default();
                let before = if kind == DocumentKind::Typst && s.scope == Scope::Code {
                    before.strip_suffix('#').unwrap_or(before)
                } else if kind == DocumentKind::Tex && s.body.starts_with('\\') {
                    before.strip_suffix('\\').unwrap_or(before)
                } else {
                    before
                };
                before.chars().all(|c| matches!(c, ' ' | '\t' | '\r'))
            })
    };
    let overrides: std::collections::BTreeSet<_> = custom
        .iter()
        .filter(eligible)
        .map(|s| s.prefix.as_str())
        .collect();
    let items: Vec<_> = custom
        .iter()
        .filter(eligible)
        .chain(
            presets
                .iter()
                .filter(eligible)
                .filter(|s| !overrides.contains(s.prefix.as_str())),
        )
        .filter_map(|s| {
            let mut start = source[..start_byte].chars().count();
            // A TeX template owns one command slash when it supplies its own.
            if kind == DocumentKind::Tex
                && s.body.starts_with('\\')
                && source[..start_byte].ends_with('\\')
            {
                let slashes = source[..start_byte]
                    .chars()
                    .rev()
                    .take_while(|c| *c == '\\')
                    .count();
                if slashes == 1 {
                    start -= 1;
                }
            }
            let wire = s.source_to_lsp().ok()?;
            Some(CompletionItem {
                origin: crate::lsp::protocol::CompletionOrigin::Snippet,
                label: s.prefix.clone(),
                detail: Some(s.description.clone()),
                documentation: Some(s.body.clone()),
                filter_text: Some(s.prefix.clone()),
                sort_text: None,
                insert_text: wire.clone(),
                insert_text_is_snippet: true,
                text_edit: Some(LspTextEdit {
                    range: LspRange {
                        start: lsp_position_at_scalar(source, ScalarOffset::new(start)),
                        end: lsp_position_at_scalar(source, ScalarOffset::new(cursor)),
                    },
                    new_text: wire,
                }),
                additional_text_edits: Vec::new(),
            })
        })
        .collect();
    (!items.is_empty()).then_some(items)
}

/// Query the trigger token, rather than its right boundary (which can already
/// be the closing delimiter). The editor supplies its revision-cached tree.
pub(crate) fn typst_context(parsed: &typst_syntax::Source, byte: usize) -> Option<Scope> {
    use typst_syntax::{LinkedNode, Side, SyntaxKind, SyntaxMode};
    let node = LinkedNode::new(parsed.root()).leaf_at(byte, Side::After)?;
    let mut ancestor = Some(&node);
    while let Some(current) = ancestor {
        if matches!(
            current.kind(),
            SyntaxKind::Raw | SyntaxKind::Str | SyntaxKind::LineComment | SyntaxKind::BlockComment
        ) {
            return None;
        }
        ancestor = current.parent();
    }
    node.mode_after().map(|mode| match mode {
        SyntaxMode::Markup => Scope::Text,
        SyntaxMode::Math => Scope::Math,
        SyntaxMode::Code => Scope::Code,
    })
}

fn inline_group_end(source: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(start) != Some(&open) {
        return None;
    }
    let mut depth = 1;
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == open {
            depth += 1;
        }
        if bytes[i] == close {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
        i += 1;
    }
    None
}

/// Structural TeX context only: no expansion, filesystem work or package IO.
/// Groups restore their enclosing mode; comments and verbatim cannot open math.
pub(crate) fn tex_context(source: &str, cursor_byte: usize) -> Option<Scope> {
    tex_context_in(source, cursor_byte, false)
}

pub(crate) fn tex_context_in(
    source: &str,
    cursor_byte: usize,
    initial_math: bool,
) -> Option<Scope> {
    let source = source.get(..cursor_byte.min(source.len()))?;
    let limit = source.len();
    let bytes = source.as_bytes();
    let mut math = initial_math;
    let mut groups = Vec::new();
    let mut environments: Vec<(&str, bool)> = Vec::new();
    let mut pending = None;
    let mut i = 0;
    while i < limit {
        match bytes[i] {
            b'%' => {
                let end = source[i..].find('\n').map_or(source.len(), |n| i + n);
                if limit <= end {
                    return None;
                }
                i = end;
            }
            b'$' => {
                math = !math;
                i += 1;
                if bytes.get(i) == Some(&b'$') {
                    i += 1;
                }
            }
            b'{' => {
                groups.push(math);
                math = pending.take().unwrap_or(math);
                i += 1;
            }
            b'}' => {
                math = groups.pop().unwrap_or(initial_math);
                pending = None;
                i += 1;
            }
            b'\\' => {
                let start = i + 1;
                let mut end = start;
                while bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
                    end += 1;
                }
                if end == start {
                    match bytes.get(start) {
                        Some(b'(' | b'[') => math = true,
                        Some(b')' | b']') => math = false,
                        _ => {}
                    }
                    i = start
                        + source
                            .get(start..)
                            .and_then(|s| s.chars().next())
                            .map_or(0, char::len_utf8);
                    continue;
                }
                let name = &source[start..end];
                i = end;
                if matches!(name, "verb" | "lstinline" | "mintinline") {
                    if bytes.get(i) == Some(&b'*') {
                        i += 1;
                    }
                    if name != "verb" && bytes.get(i) == Some(&b'[') {
                        i = inline_group_end(source, i, b'[', b']')?;
                    }
                    if name == "mintinline" {
                        i = inline_group_end(source, i, b'{', b'}')?;
                    }
                    if name != "verb" && bytes.get(i) == Some(&b'{') {
                        i = inline_group_end(source, i, b'{', b'}')?;
                        continue;
                    }
                    if let Some(delimiter) = source[i..].chars().next()
                        && !matches!(delimiter, '\n' | '\r')
                    {
                        i += delimiter.len_utf8();
                        let end = source[i..]
                            .find([delimiter, '\n', '\r'])
                            .map_or(source.len(), |n| i + n);
                        if limit <= end {
                            return None;
                        }
                        i = end;
                        if source[i..].starts_with(delimiter) {
                            i += delimiter.len_utf8();
                        }
                    }
                } else if matches!(name, "begin" | "end") {
                    let at = i + source[i..].len() - source[i..].trim_start().len();
                    if source[at..].starts_with('{')
                        && let Some(close) = source[at + 1..].find('}')
                    {
                        let after = at + close + 2;
                        let env = &source[at + 1..after - 1];
                        if name == "begin" {
                            if matches!(env, "verbatim" | "verbatim*" | "lstlisting" | "minted") {
                                let closing = format!("\\end{{{env}}}");
                                let end = source[after..]
                                    .find(&closing)
                                    .map_or(source.len(), |n| after + n);
                                if limit < end + closing.len() {
                                    return None;
                                }
                                i = end + closing.len();
                                continue;
                            }
                            environments.push((env, math));
                            if crate::editor_math_guides::math_environment(env) {
                                math = true;
                            }
                        } else if let Some(index) =
                            environments.iter().rposition(|(name, _)| *name == env)
                        {
                            math = environments[index].1;
                            environments.truncate(index);
                        }
                        i = after;
                    }
                } else if matches!(
                    name,
                    "text"
                        | "textrm"
                        | "textsf"
                        | "texttt"
                        | "textnormal"
                        | "textbf"
                        | "textit"
                        | "mbox"
                        | "hbox"
                ) {
                    pending = Some(false);
                } else if name == "ensuremath" {
                    pending = Some(true);
                }
            }
            b if b.is_ascii_whitespace() => {
                i += 1;
            }
            _ => {
                pending = None;
                i += 1;
            }
        }
    }
    Some(if math { Scope::Math } else { Scope::Text })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(source: &str) -> crate::completion_edit::SnippetExpansion {
        expand_with(source, "{{", "}}")
    }

    fn expand_with(
        source: &str,
        open: &str,
        close: &str,
    ) -> crate::completion_edit::SnippetExpansion {
        crate::completion_edit::expand_lsp_snippet(&source_to_lsp(source, open, close).unwrap())
            .unwrap()
    }

    fn sample(body: &str) -> Snippet {
        Snippet {
            prefix: "greet".into(),
            description: "Greeting".into(),
            language: Language::Both,
            body: body.into(),
            enabled: true,
            placeholder_open: default_placeholder_open(),
            placeholder_close: default_placeholder_close(),
            scope: Scope::Anywhere,
            line_start: false,
        }
    }

    #[test]
    fn ordinary_source_dollars_backslashes_old_commands_and_newlines_are_literal() {
        for source in [
            r"$x$ and $1$ and $0$",
            r"\begin{align} x & y \\ \end{align}",
            r"\$ and \{x\}",
            "#let name = \"α\"\n$ x $",
            "${USER} ${word:default} ${1:literal} ${0}",
            r"\snip{1}{literal} and #snip(1)[literal]",
            r"\field{1}{literal} and #field(1)[literal]",
        ] {
            let result = expand(source);
            assert_eq!(result.text, source);
            assert!(result.fields.is_empty());
        }
    }

    #[test]
    fn shared_fields_preserve_language_defaults_unicode_math_and_mirrors() {
        let source = r"{{1:\frac{α}{β}}} + {{1}}; {{2:$x$ \\}} {{0}}";
        let result = expand(source);
        assert_eq!(result.text, r"\frac{α}{β} + \frac{α}{β}; $x$ \\ ");
        assert_eq!(result.fields.iter().filter(|(id, _)| *id == 1).count(), 2);
        assert_eq!(result.fields.last().unwrap().0, 0);
        assert_eq!(expand("${{1:x}}$").text, "$x$");
        assert_eq!(expand("{{1:α}} + {{2:β}} + {{1}}{{0}}").text, "α + β + α");
        for language in [Language::Typst, Language::Tex, Language::Both] {
            let mut snippet = sample(source);
            snippet.language = language;
            assert_eq!(
                snippet.source_to_lsp().unwrap(),
                source_to_lsp(source, "{{", "}}").unwrap()
            );
        }
    }

    #[test]
    fn adjacent_tex_language_braces_are_not_mistaken_for_open_or_close_markers() {
        for (source, expected) in [
            (r"\begin{{{1:equation}}}{{0}}", r"\begin{equation}"),
            (r"\text{{{2:hello}}}", r"\text{hello}"),
            (r"\frac{{{1:a}}}{{{2:b}}}{{0}}", r"\frac{a}{b}"),
            (r"{{1:\frac{a}{\sqrt{b}}}}", r"\frac{a}{\sqrt{b}}"),
            (r"{{1:\{x\}}}", r"\{x\}"),
            (r"\{{1:x}}", r"\x"),
        ] {
            assert_eq!(expand(source).text, expected, "{source}");
        }
    }

    #[test]
    fn typst_brackets_quotes_commands_and_math_remain_ordinary_default_source() {
        for default in [
            "#text(fill: red)[α [β]]",
            "#let suffix = \"]\"\n#suffix",
            "https://example.com",
            "`a]b` and ``",
            "$ ] + x $",
            "/* ] /* nested ] */ */α",
            "#text[\"word]\" then \"outside\"",
        ] {
            let source = format!("{{{{1:{default}}}}}{{{{0}}}}");
            assert_eq!(expand(&source).text, default, "{source}");
        }
    }

    #[test]
    fn unicode_and_custom_delimiters_preserve_default_groups_and_field_ranges() {
        for (open, close, source, expected) in [
            ("⟪", "⟫", "⟪1:α [β]⟫ / ⟪1⟫⟪0⟫", "α [β] / α [β]"),
            (
                "<<",
                ">>",
                r"<<1:\frac{a}{b}>> + <<1>><<0>>",
                r"\frac{a}{b} + \frac{a}{b}",
            ),
            ("[[", "]]", "[[1:#text[α [β]]]][[0]]", "#text[α [β]]"),
        ] {
            let result = expand_with(source, open, close);
            assert_eq!(result.text, expected);
            for (_, field) in result.fields {
                assert!(field.start <= field.end && field.end <= expected.chars().count());
            }
        }
    }

    #[test]
    fn malformed_placeholders_report_locations_and_bound_count_and_depth() {
        for (source, expected) in [
            ("{{1:unfinished", "Close the placeholder"),
            ("{{999999999999999999999999}}", "32-bit"),
            ("{{-1}}", "positive integer"),
            ("{{word}}", "positive integer"),
            ("{{1", "close the placeholder"),
            ("{{1 default}}", "Put :"),
            ("{{0:text}}", "cannot have a default"),
            ("{{1:{unclosed}}", "Close the placeholder"),
        ] {
            let error = source_to_lsp(source, "{{", "}}").unwrap_err();
            assert!(error.contains(expected), "{source}: {error}");
            assert!(error.contains("line 1, column"), "{error}");
        }
        let deep = format!(
            "{}x{}",
            "{{1:".repeat(MAX_FIELD_DEPTH + 2),
            "}}".repeat(MAX_FIELD_DEPTH + 2)
        );
        assert!(
            source_to_lsp(&deep, "{{", "}}")
                .unwrap_err()
                .contains("nested too deeply")
        );
        assert!(
            source_to_lsp(&"{{1}} ".repeat(MAX_FIELDS + 1), "{{", "}}")
                .unwrap_err()
                .contains("4096")
        );
        assert!(
            source_to_lsp("α\n  {{no}}", "{{", "}}")
                .unwrap_err()
                .contains("line 2, column 5")
        );
    }

    #[test]
    fn marker_pairs_reject_invalid_ambiguous_and_long_delimiters() {
        for (open, close) in [
            ("", "}}"),
            ("{{", ""),
            ("words", "}}"),
            ("{{", "9"),
            ("a_", "}}"),
            ("{{", "é"),
            ("{{ ", "}}"),
            ("{{", "\n"),
            ("{{:", "}}"),
            ("\\", "}}"),
            ("{{", "{{"),
            ("<", "<<"),
            ("!", "!!!!!!!!!!!!!!!!!"),
        ] {
            assert!(
                source_to_lsp("plain", open, close).is_err(),
                "{open:?}/{close:?}"
            );
        }
        assert!(source_to_lsp("plain", "⟪", "⟫").is_ok());
    }

    #[test]
    fn rewriting_markers_preserves_literal_source_fields_and_nested_defaults() {
        for source in [
            r"\text{{{1:α}}} + {{1}} #snip(2)[literal] \snip{3} {{0}}",
            "{{1:outer {{2:inner}}}} + {{0}}",
            "{{01:x}} / {{01}} / {{0}}",
        ] {
            let rewritten = rewrite_placeholders(source, "{{", "}}", "⟪", "⟫").unwrap();
            assert_eq!(
                source_to_lsp(source, "{{", "}}").unwrap(),
                source_to_lsp(&rewritten, "⟪", "⟫").unwrap()
            );
            assert_eq!(
                rewrite_placeholders(&rewritten, "⟪", "⟫", "{{", "}}").unwrap(),
                source
            );
        }
        assert_eq!(
            rewrite_placeholders(r"\text{{{1:α}}}", "{{", "}}", "<<", ">>").unwrap(),
            r"\text{<<1:α>>}"
        );
        assert!(
            rewrite_placeholders("literal <<1:keep>> / {{1:x}}", "{{", "}}", "<<", ">>").is_err()
        );
        assert!(rewrite_placeholders("{{bad}}", "{{", "}}", "⟪", "⟫").is_err());
        assert!(rewrite_placeholders("plain", "{{", "}}", "word", ">>").is_err());
    }

    #[test]
    fn snippets_are_scoped_and_expand_through_the_completion_transaction() {
        let mut snippet = sample("Hello {{1:world}}!{{0}}");
        snippet.language = Language::Typst;
        let snippets = [snippet];
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

    #[test]
    fn nested_mirrored_defaults_are_rejected_before_any_expansion() {
        // This source stays small, while expanding its linked nested defaults
        // would duplicate their contents at every level.
        let mut body = "{{1:x}} {{1}}".to_owned();
        for id in 2..=31 {
            body = format!("{{{{{id}:{body}}}}} {{{{{id}}}}}");
        }
        assert!(body.len() < 1024);
        let snippet = sample(&body);
        assert!(
            snippet
                .source_to_lsp()
                .unwrap_err()
                .contains("Nested or overlapping")
        );
        assert!(
            validate(std::slice::from_ref(&snippet))
                .unwrap_err()
                .contains("Nested or overlapping")
        );
        assert!(items(&[snippet], DocumentKind::Typst, "gre", 3).is_none());
    }

    #[test]
    fn disabled_snippets_never_surface_or_override_enabled_presets() {
        let mut preset = sample("preset");
        let mut custom = preset.clone();
        custom.body = "custom".into();
        custom.enabled = false;
        let completion = items_from(
            std::slice::from_ref(&preset),
            std::slice::from_ref(&custom),
            DocumentKind::Typst,
            "gre",
            3,
            Some(Scope::Text),
        )
        .unwrap();
        assert_eq!(completion.len(), 1);
        assert_eq!(completion[0].documentation.as_deref(), Some("preset"));
        preset.enabled = false;
        assert!(
            items_from(
                &[preset],
                &[custom],
                DocumentKind::Typst,
                "gre",
                3,
                Some(Scope::Text)
            )
            .is_none()
        );
    }

    #[test]
    fn deserialization_defaults_enabled_state_and_placeholder_pair_without_old_aliases() {
        let mut value =
            serde_json::json!({"prefix":"x", "description":"x", "language":"tex", "body":"$x$"});
        let snippet: Snippet = serde_json::from_value(value.clone()).unwrap();
        assert!(snippet.enabled);
        assert_eq!(snippet.placeholder_open, "{{");
        assert_eq!(snippet.placeholder_close, "}}");
        value["field_command"] = serde_json::json!("snip");
        assert!(serde_json::from_value::<Snippet>(value).is_err());
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
        // LSP placeholders still insert correctly, but overlapping/coincident
        // editable fields cannot safely retain independent linked selections.
        if fields_overlap(&fields) || coincident_editable_fields(&fields) {
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
        if edits
            .windows(2)
            .any(|pair| pair[0].end > pair[1].start || pair[0].start == pair[1].start)
        {
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
            let mut mapped = shift(range.start)?;
            if *id == 0 && range.start == range.end {
                mapped += edits
                    .iter()
                    .filter(|edit| edit.is_empty() && edit.start == range.start)
                    .count()
                    * value.len();
            }
            let end = if *id == 0 && range.start == range.end {
                mapped
            } else if *id == self.active {
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

#[cfg(test)]
mod context_tests {
    use super::*;
    fn location(marked: &str) -> (String, usize) {
        let byte = marked.find('¦').unwrap();
        let cursor = marked[..byte].chars().count();
        (marked.replace('¦', ""), cursor)
    }
    fn sample(scope: Scope, line_start: bool, language: Language) -> Snippet {
        Snippet {
            prefix: "ff".into(),
            description: "Fraction".into(),
            language,
            enabled: true,
            placeholder_open: default_placeholder_open(),
            placeholder_close: default_placeholder_close(),
            body: if language == Language::Typst {
                "frac({{1:α}}, {{2:β}}){{0}}"
            } else {
                r"\frac{{{1:α}}}{{{2:β}}}{{0}}"
            }
            .into(),
            scope,
            line_start,
        }
    }
    #[test]
    fn contexts_follow_typst_syntax_and_tolerant_tex_groups() {
        for (marked, expected) in [
            ("prose ff¦", Some(Scope::Text)),
            ("$ ff¦ $", Some(Scope::Math)),
            ("$ ff¦", Some(Scope::Math)),
            ("#let value = ff¦", Some(Scope::Code)),
            ("#text[ff¦]", Some(Scope::Text)),
            ("$ #let value = ff¦ $", Some(Scope::Code)),
            ("`ff¦`", None),
            ("// ff¦", None),
            ("/* ff¦ */", None),
            ("#let x = \"ff¦\"", None),
        ] {
            let (source, cursor) = location(marked);
            let (start, _) = prefix(&source, cursor);
            assert_eq!(
                typst_context(&typst_syntax::Source::detached(&source), start),
                expected,
                "{marked}"
            );
        }
        for (marked, expected) in [
            (r"text ff¦", Some(Scope::Text)),
            (r"$ ff¦ $", Some(Scope::Math)),
            (r"$$ ff¦ $$", Some(Scope::Math)),
            (r"\( ff¦ \)", Some(Scope::Math)),
            (r"\begin{align*} ff¦ \end{align*}", Some(Scope::Math)),
            (r"\begin{Vmatrix} ff¦ \end{Vmatrix}", Some(Scope::Math)),
            (r"$ \text{ff¦} $", Some(Scope::Text)),
            (r"\ensuremath{ff¦}", Some(Scope::Math)),
            (r"$ \text{x} ff¦ $", Some(Scope::Math)),
            (r"\$ ff¦", Some(Scope::Text)),
            (r"% $ ff¦", None),
            (r"$ % ff¦", None),
            (r"\verb|$ ff¦|", None),
            (r"\lstinline[language=TeX]|code| ff¦", Some(Scope::Text)),
            (r"\lstinline[language=TeX]|ff¦|", None),
            (r"\mintinline{tex}{ff¦}", None),
            (r"\mintinline[breaklines]{tex}|ff¦|", None),
            (r"\mintinline{tex}{code} ff¦", Some(Scope::Text)),
            (r"\begin{verbatim} $ ff¦ \end{verbatim}", None),
            ("\\verb\n$ ff¦", Some(Scope::Math)),
            ("% $\nα ff¦", Some(Scope::Text)),
        ] {
            let (source, cursor) = location(marked);
            let (start, _) = prefix(&source, cursor);
            assert_eq!(tex_context(&source, start), expected, "{marked}");
        }
    }
    #[test]
    fn tex_context_reads_only_source_before_the_trigger() {
        for prefix in [
            r"text ",
            r"$ ",
            r"\begin{align} ",
            r"$ \text{",
            r"% ",
            r"\verb|",
            r"\begin{verbatim} ",
        ] {
            let expected = tex_context(prefix, prefix.len());
            for suffix in ["ff", "ff } $ \\end{align}\nprose", "% \\begin{align}\n$"] {
                let source = format!("{prefix}{suffix}");
                assert_eq!(tex_context(&source, prefix.len()), expected);
            }
        }
    }

    #[test]
    fn scopes_line_start_and_eligible_custom_override_filter_before_expansion() {
        let preset = sample(Scope::Math, false, Language::Tex);
        let mut custom = preset.clone();
        custom.body = r"custom {{1:x}}".into();
        custom.scope = Scope::Text;
        let math = items_from(
            std::slice::from_ref(&preset),
            std::slice::from_ref(&custom),
            DocumentKind::Tex,
            "$ ff",
            4,
            Some(Scope::Math),
        )
        .unwrap();
        assert_eq!(math.len(), 1);
        assert_eq!(math[0].documentation.as_deref(), Some(preset.body.as_str()));
        custom.scope = Scope::Math;
        let math = items_from(
            &[preset],
            &[custom],
            DocumentKind::Tex,
            "$ ff",
            4,
            Some(Scope::Math),
        )
        .unwrap();
        assert_eq!(math.len(), 1);
        assert_eq!(math[0].documentation.as_deref(), Some(r"custom {{1:x}}"));
        let line = sample(Scope::Text, true, Language::Tex);
        assert!(items(std::slice::from_ref(&line), DocumentKind::Tex, "text ff", 7).is_none());
        assert!(items(std::slice::from_ref(&line), DocumentKind::Tex, "  ff", 4).is_some());
        assert!(items(&[line], DocumentKind::Tex, "  \\ff", 5).is_some());
        let mut code = sample(Scope::Code, true, Language::Typst);
        code.body = "let x = {{1:1}}".into();
        assert!(items(std::slice::from_ref(&code), DocumentKind::Typst, "  #ff", 5).is_some());
        assert!(items(&[code], DocumentKind::Typst, "text #ff", 8).is_none());
        assert!(
            items(
                &[sample(Scope::Anywhere, false, Language::Both)],
                DocumentKind::Typst,
                "// ff",
                5
            )
            .is_none()
        );
    }
    #[test]
    fn tex_invocation_consumes_one_command_slash_and_reindents_linked_fields() {
        let snippet = Snippet {
            prefix: "env".into(),
            description: "Environment".into(),
            language: Language::Tex,
            enabled: true,
            placeholder_open: default_placeholder_open(),
            placeholder_close: default_placeholder_close(),
            scope: Scope::Text,
            line_start: true,
            body: "\\begin{{{1:align}}}\n  {{2:α}}\n\\end{{{1}}}{{0}}".into(),
        };
        for source in ["  env", "  \\env"] {
            let item = items(
                std::slice::from_ref(&snippet),
                DocumentKind::Tex,
                source,
                source.chars().count(),
            )
            .unwrap()
            .remove(0);
            let applied = crate::completion_edit::prepare_completion_application(
                source,
                source.chars().count(),
                &item,
            )
            .unwrap();
            assert_eq!(applied.source, "  \\begin{align}\n    α\n  \\end{align}");
            assert_eq!(applied.cursor, 9);
            for (_, range) in applied.fields.iter().filter(|(id, _)| *id == 1) {
                assert_eq!(
                    applied
                        .source
                        .chars()
                        .skip(range.start)
                        .take(range.len())
                        .collect::<String>(),
                    "align"
                );
            }
            let session = Session::new(
                tiptoptyp_core::document::DocumentKey::new(
                    tiptoptyp_core::document::WindowSessionId::new(1),
                    1,
                    1,
                ),
                &applied.source,
                applied.fields,
            )
            .unwrap();
            assert_eq!(session.selection(), 9..14);
        }
    }
    #[test]
    fn validation_rejects_nested_and_coincident_fields_but_preserves_tex_braces() {
        let mut snippet = sample(Scope::Anywhere, false, Language::Both);
        for body in ["{{1:outer {{2:inner}}}}", "{{1:{{2}}}}"] {
            snippet.body = body.into();
            assert!(
                validate(std::slice::from_ref(&snippet))
                    .unwrap_err()
                    .contains("Nested or overlapping")
            );
        }
        for body in ["{{1}}{{2}}{{0}}", "{{1}}{{1}}{{0}}"] {
            snippet.body = body.into();
            assert!(
                validate(std::slice::from_ref(&snippet))
                    .unwrap_err()
                    .contains("separators or defaults")
            );
        }
        for body in [
            r"{{1:\frac{x}{y}}} {{1}}{{0}}",
            "{{1:x}}{{2:y}}{{0}}",
            "{{1}}{{0}}",
        ] {
            snippet.body = body.into();
            validate(std::slice::from_ref(&snippet)).unwrap();
        }
    }

    #[test]
    fn single_empty_field_places_final_cursor_after_entered_text() {
        let expansion = crate::completion_edit::expand_lsp_snippet("${1}${0}").unwrap();
        let mut source = expansion.text;
        let mut session = Session::new(
            tiptoptyp_core::document::DocumentKey::new(
                tiptoptyp_core::document::WindowSessionId::new(1),
                1,
                1,
            ),
            &source,
            expansion.fields,
        )
        .unwrap();
        source.push('α');
        assert_eq!(session.update(&mut source, 1), Some(1));
        assert_eq!(session.advance(false), Some(1..1));
        assert!(session.finished());
    }

    #[test]
    fn mirrors_can_precede_their_defaults_and_touch_without_overlapping() {
        let expanded = crate::completion_edit::expand_lsp_snippet("${1}${1:α}${0}").unwrap();
        assert_eq!(expanded.text, "αα");
        let mut source = expanded.text;
        let mut session = Session::new(
            tiptoptyp_core::document::DocumentKey::new(
                tiptoptyp_core::document::WindowSessionId::new(1),
                1,
                1,
            ),
            &source,
            expanded.fields,
        )
        .unwrap();
        source.replace_range(..'α'.len_utf8(), "β");
        assert_eq!(session.update(&mut source, 1), Some(1));
        assert_eq!(source, "ββ");
        assert_eq!(session.advance(false), Some(2..2));
    }
}
