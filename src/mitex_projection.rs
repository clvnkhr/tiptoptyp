//! Checked source translation, independent of editor state and filesystem IO.
//!
//! Call `Projection::open` only on canonical Typst source. Its output is an
//! explicitly different source domain; never send that view directly to a
//! compiler, language server, or file writer. Consumers use canonical snapshots
//! and the accompanying coordinate maps at those boundaries.
use std::{
    collections::{BTreeMap, VecDeque},
    ops::Range,
};
use typst_syntax::{LinkedNode, Source, SyntaxKind, ast};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub package: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            package: "@preview/mitex:0.2.7".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidPackage,
    InvalidCanonicalSyntax,
    NativeMath { byte: usize },
    ConflictingBinding,
    UnclosedMath { byte: usize },
    InvalidGeneratedSyntax,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPackage => f.write_str("MiTeX requires a pinned @preview/mitex:major.minor.patch package"),
            Self::InvalidCanonicalSyntax => f.write_str("Fix the Typst syntax before enabling MiTeX dollar notation"),
            Self::NativeMath { byte } => write!(f, "Native Typst math at byte {byte} makes MiTeX dollar notation ambiguous"),
            Self::ConflictingBinding => f.write_str("A MiTeX renderer binding is missing, shadowed, renamed, or imported from another scope/package"),
            Self::UnclosedMath { byte } => write!(f, "Close the TeX dollar expression at byte {byte} before saving"),
            Self::InvalidGeneratedSyntax => f.write_str("MiTeX translation would produce invalid Typst; no output was accepted"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone)]
struct Span {
    input: Range<usize>,
    output: Range<usize>,
}

/// Text and a monotone byte-coordinate map. Copied text maps exactly. A
/// generated wrapper or escape maps to the corresponding boundary/character.
/// Invalid byte offsets (including the middle of UTF-8) return `None`.
#[derive(Debug, Clone)]
pub struct Translation {
    input: String,
    output: String,
    spans: Vec<Span>,
}
impl Translation {
    pub fn input(&self) -> &str {
        &self.input
    }
    pub fn output(&self) -> &str {
        &self.output
    }
    pub fn input_to_output(&self, byte: usize) -> Option<usize> {
        self.map(byte, false)
    }
    pub fn output_to_input(&self, byte: usize) -> Option<usize> {
        self.map(byte, true)
    }
    fn map(&self, byte: usize, reverse: bool) -> Option<usize> {
        let (from, to) = if reverse {
            (&self.output, &self.input)
        } else {
            (&self.input, &self.output)
        };
        if !from.is_char_boundary(byte) {
            return None;
        }
        if byte == from.len() {
            return Some(to.len());
        }
        let index = self
            .spans
            .partition_point(|span| {
                (if reverse {
                    span.output.start
                } else {
                    span.input.start
                }) <= byte
            })
            .checked_sub(1)?;
        let span = &self.spans[index];
        let (a, b) = if reverse {
            (&span.output, &span.input)
        } else {
            (&span.input, &span.output)
        };
        let mapped = if a.len() == b.len() {
            b.start + byte - a.start
        } else if byte >= a.end {
            b.end
        } else {
            b.start
        };
        to.is_char_boundary(mapped).then_some(mapped)
    }
    fn new(input: &str) -> Self {
        Self {
            input: input.into(),
            output: String::new(),
            spans: Vec::new(),
        }
    }
    fn push(&mut self, input: Range<usize>, text: &str) {
        let start = self.output.len();
        self.output.push_str(text);
        self.spans.push(Span {
            input,
            output: start..self.output.len(),
        });
    }
    fn copy(&mut self, range: Range<usize>) {
        let start = self.output.len();
        self.output.push_str(&self.input[range.clone()]);
        self.spans.push(Span {
            input: range,
            output: start..self.output.len(),
        });
    }
}

struct Original {
    payload: String,
    spelling: String,
    pieces: Vec<(Range<usize>, String)>,
}

/// A validated original document and its editable dollar notation. Untouched
/// calls preserve their original literal spelling, whitespace, and options.
pub struct Projection {
    config: Config,
    view: Translation,
    originals: Vec<Original>,
}
impl Projection {
    /// Validate an existing syntax tree without constructing translated text
    /// or source maps. UI consumers cache this result by revision and config.
    pub fn compatible(parsed: &Source, config: &Config) -> bool {
        if validate_config(config).is_err() || parsed.root().diagnosis().errors {
            return false;
        }
        let mut facts = Facts {
            validation_only: true,
            ..Facts::default()
        };
        inspect(LinkedNode::new(parsed.root()), config, &mut facts);
        facts.math.is_empty()
            && !facts.conflict
            && (!facts.needs_inline || facts.imported)
            && (!facts.needs_block || facts.imported_block)
    }
    pub fn open(source: &str, config: Config) -> Result<Self, Error> {
        validate_config(&config)?;
        let parsed = Source::detached(source);
        let mut facts = Facts::default();
        inspect(LinkedNode::new(parsed.root()), &config, &mut facts);
        if let Some(byte) = facts.math.keys().next() {
            return Err(Error::NativeMath { byte: *byte });
        }
        if parsed.root().diagnosis().errors {
            return Err(Error::InvalidCanonicalSyntax);
        }
        if facts.conflict {
            return Err(Error::ConflictingBinding);
        }
        let mut view = Translation::new(source);
        let mut originals = Vec::new();
        let mut last = 0;
        for call in facts.calls {
            let block = call
                .children()
                .find(|child| !child.kind().is_trivia())
                .is_some_and(|callee| callee.leaf_text() == "mitex");
            if !(if block {
                facts.imported_block
            } else {
                facts.imported
            }) {
                return Err(Error::ConflictingBinding);
            }
            let Some((range, mut payload, mut pieces)) = simple_call(&call, source) else {
                continue;
            };
            if !block && display_payload(&payload) {
                continue;
            }
            if block && !display_payload(&payload) {
                let start = pieces.first().map_or(range.end, |(part, _)| part.start);
                let end = pieces.last().map_or(start, |(part, _)| part.end);
                pieces.insert(0, (start..start, " ".into()));
                pieces.push((end..end, " ".into()));
                payload = format!(" {payload} ");
            }
            // A TeX payload with its own dollar math cannot be represented by
            // one unambiguous outer pair. Keep that explicit mi(...) call.
            if closing_dollar(&format!("${payload}$"), 0) != Some(payload.len() + 1) {
                continue;
            }
            view.copy(last..range.start);
            let payload_start = pieces.first().map_or(range.end, |(part, _)| part.start);
            let payload_end = pieces.last().map_or(payload_start, |(part, _)| part.end);
            view.push(range.start..payload_start, "$");
            for (part, text) in &pieces {
                view.push(part.clone(), text);
            }
            view.push(payload_end..range.end, "$");
            originals.push(Original {
                payload,
                spelling: source[range.clone()].into(),
                pieces: pieces
                    .into_iter()
                    .map(|(part, text)| (part.start - range.start..part.end - range.start, text))
                    .collect(),
            });
            last = range.end;
        }
        view.copy(last..source.len());
        Ok(Self {
            config,
            view,
            originals,
        })
    }
    pub fn view(&self) -> &Translation {
        &self.view
    }

    /// Translate a complete view for persistence/services. An unfinished pair
    /// is an error, never silently persisted as native Typst mathematics.
    pub fn encode(&self, view: &str) -> Result<Translation, Error> {
        if view == self.view.output {
            return Ok(Translation {
                input: view.into(),
                output: self.view.input.clone(),
                spans: self
                    .view
                    .spans
                    .iter()
                    .map(|span| Span {
                        input: span.output.clone(),
                        output: span.input.clone(),
                    })
                    .collect(),
            });
        }
        let parsed = Source::detached(view);
        let mut facts = Facts::default();
        inspect(LinkedNode::new(parsed.root()), &self.config, &mut facts);
        if facts.conflict {
            return Err(Error::ConflictingBinding);
        }
        let mut blocks = Vec::new();
        let mut i = 0;
        let mut protected_index = 0;
        while i < view.len() {
            while facts
                .protected
                .get(protected_index)
                .is_some_and(|range| range.end <= i)
            {
                protected_index += 1;
            }
            if let Some(range) = facts
                .protected
                .get(protected_index)
                .filter(|range| range.contains(&i))
            {
                i = range.end;
                continue;
            }
            let ch = view[i..].chars().next().unwrap();
            if ch == '\\' {
                i += 1;
                if i < view.len() {
                    i += view[i..].chars().next().unwrap().len_utf8();
                }
            } else if ch == '$' {
                let end = closing_dollar(view, i).ok_or(Error::UnclosedMath { byte: i })?;
                blocks.push(i..end + 1);
                i = end + 1;
            } else {
                i += ch.len_utf8();
            }
        }
        let mut original: BTreeMap<&str, VecDeque<&Original>> = BTreeMap::new();
        // Explicit calls without shorthand do not trigger import insertion.
        // Apply the same missing-binding guard as `open`, so a successful
        // save can always be reopened/reverted in projected mode.
        for call in &facts.calls {
            let block = call
                .children()
                .find(|child| !child.kind().is_trivia())
                .is_some_and(|callee| callee.leaf_text() == "mitex");
            if !(if block {
                facts.imported_block
            } else {
                facts.imported
            }) && blocks.is_empty()
            {
                return Err(Error::ConflictingBinding);
            }
        }
        for item in &self.originals {
            original.entry(&item.payload).or_default().push_back(item);
        }
        let mut translated = Translation::new(view);
        let needs_inline = facts
            .calls
            .iter()
            .any(|call| call.children().any(|child| child.leaf_text() == "mi"))
            || blocks
                .iter()
                .any(|range| !display_payload(&view[range.start + 1..range.end - 1]));
        let needs_block = facts
            .calls
            .iter()
            .any(|call| call.children().any(|child| child.leaf_text() == "mitex"))
            || blocks
                .iter()
                .any(|range| display_payload(&view[range.start + 1..range.end - 1]));
        let mut imports = Vec::new();
        if needs_inline && !facts.imported {
            imports.push("mi");
        }
        if needs_block && !facts.imported_block {
            imports.push("mitex");
        }
        if !imports.is_empty() {
            translated.push(
                0..0,
                &format!(
                    "#import \"{}\": {}\n",
                    self.config.package,
                    imports.join(", ")
                ),
            );
        }
        let mut last = 0;
        for range in blocks {
            translated.copy(last..range.start);
            let payload = &view[range.start + 1..range.end - 1];
            if let Some(old) = original.get_mut(payload).and_then(VecDeque::pop_front) {
                let first = old
                    .pieces
                    .first()
                    .map_or(old.spelling.len(), |(part, _)| part.start);
                let mut payload_byte = range.start + 1;
                translated.push(range.start..payload_byte, &old.spelling[..first]);
                let mut end = first;
                for (part, decoded) in &old.pieces {
                    translated.push(
                        payload_byte..payload_byte + decoded.len(),
                        &old.spelling[part.clone()],
                    );
                    payload_byte += decoded.len();
                    end = part.end;
                }
                translated.push(payload_byte..range.end, &old.spelling[end..]);
            } else {
                let markup = facts.math.get(&range.start).copied().unwrap_or(true);
                translated.push(
                    range.start..range.start + 1,
                    match (markup, display_payload(payload)) {
                        (true, false) => "#mi(\"",
                        (false, false) => "mi(\"",
                        (true, true) => "#mitex(\"",
                        (false, true) => "mitex(\"",
                    },
                );
                for (byte, ch) in payload.char_indices() {
                    let escaped = match ch {
                        '\\' => "\\\\".into(),
                        '"' => "\\\"".into(),
                        '\r' => "\\r".into(),
                        '\t' => "\\t".into(),
                        _ => ch.to_string(),
                    };
                    let start = range.start + 1 + byte;
                    translated.push(start..start + ch.len_utf8(), &escaped);
                }
                translated.push(range.end - 1..range.end, "\")");
            }
            last = range.end;
        }
        translated.copy(last..view.len());
        let generated = Source::detached(translated.output.clone());
        if generated.root().diagnosis().errors {
            return Err(Error::InvalidGeneratedSyntax);
        }
        Ok(translated)
    }
}

fn validate_config(config: &Config) -> Result<(), Error> {
    let version = config
        .package
        .strip_prefix("@preview/mitex:")
        .ok_or(Error::InvalidPackage)?;
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(Error::InvalidPackage);
    }
    Ok(())
}

fn display_payload(payload: &str) -> bool {
    payload.starts_with(char::is_whitespace) && payload.ends_with(char::is_whitespace)
}

/// TeX payloads in the displayed buffer, including an unfinished final island.
/// Reuses the editor's parse and excludes dollars in Typst literals/comments.
pub fn dollar_payloads(parsed: &Source) -> Vec<Range<usize>> {
    let source = parsed.text();
    let mut facts = Facts::default();
    inspect(
        LinkedNode::new(parsed.root()),
        &Config::default(),
        &mut facts,
    );
    let mut ranges = Vec::new();
    let mut i = 0;
    let mut protected = 0;
    while i < source.len() {
        while facts
            .protected
            .get(protected)
            .is_some_and(|range| range.end <= i)
        {
            protected += 1;
        }
        if let Some(range) = facts
            .protected
            .get(protected)
            .filter(|range| range.contains(&i))
        {
            i = range.end;
            continue;
        }
        match source.as_bytes()[i] {
            b'\\' => {
                i += 1;
                if i < source.len() {
                    i += source[i..].chars().next().unwrap().len_utf8();
                }
            }
            b'$' => {
                let end = closing_dollar(source, i).unwrap_or(source.len());
                ranges.push(i + 1..end);
                i = end.saturating_add(1);
            }
            _ => i += source[i..].chars().next().unwrap().len_utf8(),
        }
    }
    ranges
}

#[derive(Default)]
struct Facts<'a> {
    validation_only: bool,
    needs_inline: bool,
    needs_block: bool,
    imported: bool,
    imported_block: bool,
    conflict: bool,
    math: BTreeMap<usize, bool>,
    protected: Vec<Range<usize>>,
    calls: Vec<LinkedNode<'a>>,
}

fn contains_ident(node: &LinkedNode<'_>, name: &str) -> bool {
    (node.kind() == SyntaxKind::Ident && node.leaf_text() == name)
        || node.children().any(|child| contains_ident(&child, name))
}
fn inspect<'a>(node: LinkedNode<'a>, config: &Config, facts: &mut Facts<'a>) {
    match node.kind() {
        SyntaxKind::Equation => {
            facts.math.insert(
                node.offset(),
                node.parent()
                    .is_some_and(|parent| parent.kind() == SyntaxKind::Markup),
            );
            return;
        }
        SyntaxKind::Str | SyntaxKind::Raw | SyntaxKind::LineComment | SyntaxKind::BlockComment => {
            if !facts.validation_only {
                facts.protected.push(node.range());
            }
            return;
        }
        SyntaxKind::ModuleImport => {
            if let Some(import) = node.get().cast::<ast::ModuleImport>()
                && import.imports().is_none()
                && (import
                    .new_name()
                    .is_some_and(|name| matches!(name.get().as_str(), "mi" | "mitex"))
                    || (import.new_name().is_none()
                        && import
                            .bare_name()
                            .is_ok_and(|name| matches!(name.as_str(), "mi" | "mitex"))))
            {
                facts.conflict = true;
            }
            let package = node
                .children()
                .find(|child| child.kind() == SyntaxKind::Str)
                .and_then(|child| child.get().cast::<ast::Str>())
                .map(|s| s.get().to_string());
            let wildcard = node
                .children()
                .any(|child| child.kind() == SyntaxKind::Star);
            let items = node
                .children()
                .find(|child| child.kind() == SyntaxKind::ImportItems);
            let imports_mi = wildcard
                || items
                    .as_ref()
                    .is_some_and(|items| contains_ident(items, "mi"));
            let imports_block = wildcard
                || items
                    .as_ref()
                    .is_some_and(|items| contains_ident(items, "mitex"));
            if imports_mi || imports_block {
                let top_level = node.parent().is_some_and(|parent| {
                    parent.kind() == SyntaxKind::Markup && parent.parent().is_none()
                });
                let renamed = items.is_some_and(|items| {
                    items
                        .children()
                        .any(|item| item.kind() == SyntaxKind::RenamedImportItem)
                });
                if package.as_deref() == Some(config.package.as_str()) && top_level && !renamed {
                    facts.imported |= imports_mi;
                    facts.imported_block |= imports_block;
                } else {
                    facts.conflict = true;
                }
            }
        }
        SyntaxKind::LetBinding => {
            if node.get().cast::<ast::LetBinding>().is_some_and(|binding| {
                binding
                    .kind()
                    .bindings()
                    .iter()
                    .any(|name| matches!(name.get().as_str(), "mi" | "mitex"))
            }) {
                facts.conflict = true;
            }
        }
        SyntaxKind::Closure => {
            // Fail closed on possible shadowing; name resolution belongs to
            // the language server, not this local transformation layer.
            if node
                .children()
                .find(|child| child.kind() == SyntaxKind::Params)
                .is_some_and(|params| {
                    contains_ident(&params, "mi") || contains_ident(&params, "mitex")
                })
            {
                facts.conflict = true;
            }
        }
        SyntaxKind::FuncCall
            if node
                .children()
                .find(|child| !child.kind().is_trivia())
                .is_some_and(|callee| {
                    callee.kind() == SyntaxKind::Ident
                        && matches!(callee.leaf_text().as_str(), "mi" | "mitex")
                }) =>
        {
            let inline = node
                .children()
                .find(|child| !child.kind().is_trivia())
                .is_some_and(|callee| callee.leaf_text() == "mi");
            facts.needs_inline |= inline;
            facts.needs_block |= !inline;
            if !facts.validation_only {
                facts.calls.push(node.clone());
            }
        }
        _ => {}
    }
    for child in node.children() {
        inspect(child, config, facts);
    }
}

type Pieces = Vec<(Range<usize>, String)>;
fn simple_call(node: &LinkedNode<'_>, source: &str) -> Option<(Range<usize>, String, Pieces)> {
    // Only markup calls can be presented without changing their code context.
    if node.parent()?.kind() != SyntaxKind::Markup {
        return None;
    }
    let range = node.range();
    if range.start == 0 || source.as_bytes()[range.start - 1] != b'#' {
        return None;
    }
    let args = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Args)?;
    let mut values = args.children().filter(|child| {
        !child.kind().is_trivia()
            && !matches!(
                child.kind(),
                SyntaxKind::LeftParen | SyntaxKind::RightParen | SyntaxKind::Comma
            )
    });
    let literal = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let pieces = match literal.kind() {
        SyntaxKind::Str => decode_string(source, literal.range()),
        // Single-backtick raw strings preserve their payload verbatim; block
        // raw literals have indentation trimming and stay explicit for now.
        SyntaxKind::Raw
            if source[literal.range()].starts_with('`')
                && !source[literal.range()].starts_with("``") =>
        {
            let payload = literal.offset() + 1..literal.range().end - 1;
            source[payload.clone()]
                .char_indices()
                .map(|(byte, ch)| {
                    let start = payload.start + byte;
                    (start..start + ch.len_utf8(), ch.to_string())
                })
                .collect()
        }
        _ => return None,
    };
    let payload = pieces.iter().map(|(_, text)| text.as_str()).collect();
    Some((range.start - 1..range.end, payload, pieces))
}

fn decode_string(source: &str, range: Range<usize>) -> Pieces {
    let mut pieces = Vec::new();
    let mut i = range.start + 1;
    while i < range.end - 1 {
        let start = i;
        let ch = source[i..].chars().next().unwrap();
        i += ch.len_utf8();
        if ch == '\\' && i < range.end - 1 {
            let next = source[i..].chars().next().unwrap();
            let decoded = match next {
                '\\' => Some('\\'),
                '"' => Some('"'),
                'n' => Some('\n'),
                'r' => Some('\r'),
                't' => Some('\t'),
                _ => None,
            };
            if let Some(decoded) = decoded {
                i += next.len_utf8();
                pieces.push((start..i, decoded.to_string()));
                continue;
            }
            if source[i..].starts_with("u{")
                && let Some(end) = source[i + 2..].find('}')
            {
                let end = i + 2 + end;
                if let Some(decoded) = u32::from_str_radix(&source[i + 2..end], 16)
                    .ok()
                    .and_then(char::from_u32)
                {
                    i = end + 1;
                    pieces.push((start..i, decoded.to_string()));
                    continue;
                }
            }
        }
        pieces.push((start..i, ch.to_string()));
    }
    pieces
}

fn closing_dollar(source: &str, opening: usize) -> Option<usize> {
    let mut chars = source[opening + 1..].char_indices();
    let mut comment = false;
    while let Some((offset, ch)) = chars.next() {
        if comment {
            if matches!(ch, '\n' | '\r') {
                comment = false;
            }
            continue;
        }
        match ch {
            '\\' => {
                chars.next();
            }
            '%' => comment = true,
            '$' => return Some(opening + 1 + offset),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    const IMPORT: &str = "#import \"@preview/mitex:0.2.7\": mi\n";
    #[test]
    fn refuses_real_typst_math_but_not_dollars_in_literals_and_comments() {
        for source in ["$x$", "#let x = $x$", "Text $\n x\n$", "$"] {
            assert!(Projection::open(source, Config::default()).is_err());
        }
        for source in ["`$x$`", "// $x$", "#let s = \"$x$\"", "/* $x$ */"] {
            let p = Projection::open(source, Config::default()).unwrap();
            assert_eq!(p.view().output(), source);
            assert_eq!(p.encode(source).unwrap().output(), source);
        }
    }
    #[test]
    fn preserves_original_spelling_even_when_other_text_is_edited() {
        let source = format!("{IMPORT}A #mi( `\\alpha` ) and #mi(\"\\\\beta\") end.");
        let p = Projection::open(&source, Config::default()).unwrap();
        assert_eq!(
            p.view().output(),
            format!("{IMPORT}A $\\alpha$ and $\\beta$ end.")
        );
        assert_eq!(p.encode(p.view().output()).unwrap().output(), source);
        assert_eq!(
            p.encode(&p.view().output().replace("end.", "changed."))
                .unwrap()
                .output(),
            source.replace("end.", "changed.")
        );
    }
    #[test]
    fn encodes_new_tex_safely_and_inserts_exactly_one_import() {
        let p = Projection::open("Hello", Config::default()).unwrap();
        let view = "Hello $\\alpha + \\\"quoted\\\"$ and $\n\t\\beta\n$";
        let encoded = p.encode(view).unwrap();
        let import = "#import \"@preview/mitex:0.2.7\": mi, mitex\n";
        assert!(encoded.output().starts_with(import));
        assert_eq!(encoded.output().matches("#import").count(), 1);
        assert!(encoded.output().contains("#mitex(\"\n\\t\\\\beta\n\")"));
        let reopened = Projection::open(encoded.output(), Config::default()).unwrap();
        assert_eq!(reopened.view().output(), format!("{import}{view}"));
        assert_eq!(
            reopened.encode(reopened.view().output()).unwrap().output(),
            encoded.output()
        );
    }
    #[test]
    fn display_math_uses_block_renderer_and_preserves_original_calls() {
        let p = Projection::open("", Config::default()).unwrap();
        for (view, renderer) in [
            ("$x$", "mi"),
            ("$ x$", "mi"),
            ("$x $", "mi"),
            ("$ x $", "mitex"),
            ("$\n  \\alpha\n$", "mitex"),
        ] {
            let encoded = p.encode(view).unwrap();
            assert!(
                encoded.output().contains(&format!("#{renderer}(\"")),
                "{}",
                encoded.output()
            );
            let reopened = Projection::open(encoded.output(), Config::default()).unwrap();
            assert!(reopened.view().output().ends_with(view));
        }
        for payload in ["x", "", " x ", "\\\\alpha"] {
            let original =
                format!("#import \"@preview/mitex:0.2.7\": mitex\n#mitex(\"{payload}\") tail");
            let p = Projection::open(&original, Config::default()).unwrap();
            assert!(p.view().output().contains("$ ") || p.view().output().contains("$\n"));
            assert_eq!(p.encode(p.view().output()).unwrap().output(), original);
            assert_eq!(
                p.encode(&p.view().output().replace("tail", "next"))
                    .unwrap()
                    .output(),
                original.replace("tail", "next")
            );
            for byte in p.view().input().char_indices().map(|(byte, _)| byte) {
                assert!(
                    p.view()
                        .output()
                        .is_char_boundary(p.view().input_to_output(byte).unwrap())
                );
            }
        }
        let encoded = p.encode("#mi(`x`) $ y $").unwrap();
        assert!(
            encoded
                .output()
                .starts_with("#import \"@preview/mitex:0.2.7\": mi, mitex")
        );
        assert!(Projection::open(encoded.output(), Config::default()).is_ok());
    }

    #[test]
    fn compatibility_matches_open_without_building_a_projection() {
        for source in [
            "",
            "plain text",
            "$native$",
            "#let x = (",
            "#let mi(x) = x",
            "#mi(`x`)",
            "#import \"other.typ\": *",
            "#import \"@preview/mitex:0.2.7\": mi\n#mi(`x`)",
            "#import \"@preview/mitex:0.2.7\": mitex\n#mitex(`x`)",
            "#import \"@preview/mitex:0.2.6\": mi\n#mi(`x`)",
            "`$literal$` // $comment$\n",
        ] {
            let parsed = Source::detached(source);
            assert_eq!(
                Projection::compatible(&parsed, &Config::default()),
                Projection::open(source, Config::default()).is_ok(),
                "{source}"
            );
            let mut facts = Facts {
                validation_only: true,
                ..Facts::default()
            };
            inspect(
                LinkedNode::new(parsed.root()),
                &Config::default(),
                &mut facts,
            );
            assert!(facts.protected.is_empty());
            assert!(facts.calls.is_empty());
        }
    }
    #[test]
    fn escaped_and_commented_tex_dollars_do_not_close_the_island() {
        let p = Projection::open("", Config::default()).unwrap();
        let view = "$\\$ + x % $ ignored\n + y$";
        let text = p.encode(view).unwrap();
        assert_eq!(
            Projection::open(text.output(), Config::default())
                .unwrap()
                .view()
                .output(),
            format!("{IMPORT}{view}")
        );
        assert!(matches!(
            p.encode("$unfinished"),
            Err(Error::UnclosedMath { byte: 0 })
        ));
    }
    #[test]
    fn binding_conflicts_and_import_injection_fail_closed() {
        for source in [
            "#let mi(x) = x",
            "#import \"other.typ\": mi",
            "#import \"other.typ\": *",
            "#mi(`x`)",
        ] {
            assert!(
                Projection::open(source, Config::default()).is_err(),
                "{source}"
            );
        }
        assert!(
            Projection::open(
                "",
                Config {
                    package: "@preview/mitex:0.2.7\"\n#evil()".into()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn renamed_imports_cannot_bypass_binding_validation_with_whitespace() {
        for source in [
            "#import \"@preview/mitex:0.2.7\": mi\tas\tother",
            "#import \"other.typ\" as mi",
            "#import \"mi.typ\"",
        ] {
            assert!(
                matches!(
                    Projection::open(source, Config::default()),
                    Err(Error::ConflictingBinding)
                ),
                "{source}"
            );
        }
    }
    #[test]
    fn unicode_and_escape_coordinates_map_in_both_directions() {
        let source = format!("{IMPORT}😀 #mi(\"\\u{{3b1}} + \\\\beta\") tail");
        let p = Projection::open(&source, Config::default()).unwrap();
        let view = p.view().output();
        assert!(view.contains("$α + \\beta$"));
        for token in ["😀", " + ", "beta", "tail"] {
            let input = source.find(token).unwrap();
            let output = view.find(token).unwrap();
            assert_eq!(p.view().input_to_output(input), Some(output));
            assert_eq!(p.view().output_to_input(output), Some(input));
        }
        assert_eq!(
            p.view().output_to_input(view.find('α').unwrap()),
            Some(source.find("\\u{").unwrap())
        );
        assert_eq!(
            p.view().input_to_output(source.find('😀').unwrap() + 1),
            None
        );
        let encoded = p.encode(view).unwrap();
        assert_eq!(encoded.output(), source);
        assert_eq!(
            encoded.input_to_output(view.find("beta").unwrap()),
            Some(source.find("beta").unwrap())
        );
    }
    #[test]
    fn keeps_nonrepresentable_calls_explicit() {
        for call in [
            "#mi(`$x$`)",
            "#mi(\"x\", scope: (:))",
            "#mi(```tex\n x\n```)",
        ] {
            let source = format!("{IMPORT}{call}");
            let p = Projection::open(&source, Config::default()).unwrap();
            assert_eq!(p.view().output(), source);
            assert_eq!(p.encode(&source).unwrap().output(), source);
        }
    }

    #[test]
    fn typst_unknown_string_escapes_keep_their_tex_backslash() {
        let source = format!("{IMPORT}{}", r#"#mi("\alpha + \epsilon")"#);
        let p = Projection::open(&source, Config::default()).unwrap();
        assert_eq!(p.view().output(), format!("{IMPORT}$\\alpha + \\epsilon$"));
        assert_eq!(p.encode(p.view().output()).unwrap().output(), source);
    }

    #[test]
    fn all_mapped_boundaries_are_valid_monotone_and_cover_the_endpoints() {
        let p = Projection::open("", Config::default()).unwrap();
        for view in [
            "",
            "😀 text",
            "文 $\\alpha + β$ tail",
            "$a$ $b$",
            "$\n\\text{\"😀\"}\n$",
        ] {
            let t = p.encode(view).unwrap();
            for reverse in [false, true] {
                let from = if reverse { t.output() } else { t.input() };
                let to = if reverse { t.input() } else { t.output() };
                let mut previous = 0;
                for byte in from
                    .char_indices()
                    .map(|(byte, _)| byte)
                    .chain(std::iter::once(from.len()))
                {
                    let mapped = if reverse {
                        t.output_to_input(byte)
                    } else {
                        t.input_to_output(byte)
                    }
                    .unwrap();
                    assert!(
                        mapped >= previous && to.is_char_boundary(mapped),
                        "{view:?} at {byte}"
                    );
                    previous = mapped;
                }
                assert_eq!(previous, to.len());
            }
        }
    }

    #[test]
    fn copied_text_uses_constant_map_space_and_existing_import_is_not_duplicated() {
        let source = "plain text\n".repeat(10_000);
        let p = Projection::open(&source, Config::default()).unwrap();
        assert_eq!(p.view.spans.len(), 1);
        assert_eq!(p.encode(&source).unwrap().spans.len(), 1);
        let source = format!("{IMPORT}Text");
        let p = Projection::open(&source, Config::default()).unwrap();
        let result = p.encode(&format!("{source} $\\alpha$")).unwrap();
        assert_eq!(result.output().matches(IMPORT).count(), 1);
    }

    #[test]
    fn code_values_do_not_count_as_shadowing_but_parameters_do() {
        let source = format!("{IMPORT}#let x = mi(\"x\")");
        assert!(Projection::open(&source, Config::default()).is_ok());
        let source = format!("{IMPORT}#let f(mi) = mi(\"x\")");
        assert!(matches!(
            Projection::open(&source, Config::default()),
            Err(Error::ConflictingBinding)
        ));
        let p = Projection::open("", Config::default()).unwrap();
        assert_eq!(
            p.encode("#let x = $\\alpha$").unwrap().output(),
            format!("{IMPORT}#let x = mi(\"\\\\alpha\")")
        );
    }

    #[test]
    fn explicit_calls_without_an_import_cannot_create_an_unreopenable_save() {
        let projection = Projection::open("", Config::default()).unwrap();
        assert!(matches!(
            projection.encode("#mi(\"x\")"),
            Err(Error::ConflictingBinding)
        ));
        let encoded = projection.encode("#mi(\"x\") $y$").unwrap();
        assert!(Projection::open(encoded.output(), Config::default()).is_ok());
        let imported = format!("{IMPORT}#mi(\"x\")");
        assert!(projection.encode(&imported).is_ok());
    }
}
