//! Snapshot-bound, token-preserving refactorings for literal local TeX macros.
//!
//! This is deliberately not a TeX interpreter. Unsupported execution, argument
//! consumers and token inspection fail closed. In particular, adding a group
//! around an expansion is not safe: it changes which atom owns a math script.
use std::{collections::BTreeMap, ops::Range};

use crate::delimiters::{TexDelimiterIndex, tex_ignored_ranges};

const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOKENS: usize = 500_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InlineScope {
    Occurrence,
    AllOccurrences,
}

#[derive(Debug)]
pub(crate) struct Plan {
    original: String,
    replacement: String,
    selection: Range<usize>,
    label: String,
    occurrences: usize,
}

#[derive(Debug)]
pub(crate) struct Applied {
    pub(crate) source: String,
    pub(crate) selection: Range<usize>,
}

impl Plan {
    pub(crate) fn preview(&self) -> &str {
        &self.replacement
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn occurrences(&self) -> usize {
        self.occurrences
    }

    pub(crate) fn apply(&self, current: &str) -> Result<Applied, String> {
        if current != self.original {
            return Err("The document changed. Create a new refactoring preview.".into());
        }
        Ok(Applied {
            source: self.replacement.clone(),
            selection: self.selection.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Word(String),
    Symbol(char),
    Character(char),
    Open,
    Close,
    Space,
    Paragraph,
    Opaque,
}

impl Kind {
    fn word(&self) -> Option<&str> {
        if let Self::Word(name) = self {
            Some(name)
        } else {
            None
        }
    }

    fn letter(&self) -> bool {
        matches!(self, Self::Character(ch) if ch.is_alphabetic())
    }
}

#[derive(Clone, Debug)]
struct Token {
    kind: Kind,
    bytes: Range<usize>,
    /// Includes trivia discarded while TeX is reading a control word.
    consumed_end: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mouth {
    NewLine,
    MidLine,
    SkipBlanks,
}

fn lex(source: &str) -> Result<Vec<Token>, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err("TeX refactoring currently supports documents up to 4 MiB.".into());
    }
    // TeX's ^^ translation precedes comment/literal handling and can even
    // manufacture a backslash, comment marker or newline. Do not guess at it.
    if source.contains("^^") {
        return Err("TeX ^^ character translation needs manual refactoring.".into());
    }
    let ignored = tex_ignored_ranges(source);
    let mut ignored_at = 0;
    let mut at = 0;
    let mut mouth = Mouth::NewLine;
    let mut tokens: Vec<Token> = Vec::new();
    while at < source.len() {
        if tokens.len() >= MAX_TOKENS {
            return Err("This document has too many tokens for a safe refactoring.".into());
        }
        while ignored.get(ignored_at).is_some_and(|range| range.end <= at) {
            ignored_at += 1;
        }
        let start = at;
        let ch = source[at..].chars().next().unwrap();
        let kind = if let Some(range) = ignored.get(ignored_at).filter(|r| r.start == at) {
            at = range.end;
            if ch == '%' {
                // A comment removes its line ending, rather than adding a space.
                if source.as_bytes().get(at) == Some(&b'\r') {
                    at += 1;
                }
                if source.as_bytes().get(at) == Some(&b'\n') {
                    at += 1;
                }
                mouth = Mouth::NewLine;
                extend_discarded_trivia(&mut tokens, start, at);
                continue;
            }
            mouth = Mouth::MidLine;
            Kind::Opaque
        } else if ch == '\\' {
            at += 1;
            let name_start = at;
            while source
                .as_bytes()
                .get(at)
                .is_some_and(u8::is_ascii_alphabetic)
            {
                at += 1;
            }
            if at != name_start {
                if source[at..]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_alphabetic() && !ch.is_ascii())
                {
                    return Err("Unicode control-word catcodes differ between TeX engines. Refactor this command manually.".into());
                }
                mouth = Mouth::SkipBlanks;
                Kind::Word(source[name_start..at].into())
            } else {
                let symbol = source[at..]
                    .chars()
                    .next()
                    .ok_or("A trailing backslash must be completed before refactoring.")?;
                if symbol.is_whitespace() || symbol.is_alphabetic() && !symbol.is_ascii() {
                    return Err("Explicit control spaces and Unicode command names need manual refactoring.".into());
                }
                at += symbol.len_utf8();
                mouth = Mouth::MidLine;
                Kind::Symbol(symbol)
            }
        } else if matches!(ch, '\n' | '\r') {
            at += ch.len_utf8();
            if ch == '\r' && source.as_bytes().get(at) == Some(&b'\n') {
                at += 1;
            }
            let kind = match mouth {
                Mouth::MidLine => Some(Kind::Space),
                Mouth::NewLine => Some(Kind::Paragraph),
                Mouth::SkipBlanks => None,
            };
            mouth = Mouth::NewLine;
            let Some(kind) = kind else {
                extend_discarded_trivia(&mut tokens, start, at);
                continue;
            };
            kind
        } else if matches!(ch, ' ' | '\t') {
            at += 1;
            if mouth != Mouth::MidLine {
                extend_discarded_trivia(&mut tokens, start, at);
                continue;
            }
            mouth = Mouth::SkipBlanks;
            Kind::Space
        } else {
            at += ch.len_utf8();
            mouth = Mouth::MidLine;
            match ch {
                '{' => Kind::Open,
                '}' => Kind::Close,
                ch => Kind::Character(ch),
            }
        };
        tokens.push(Token {
            kind,
            bytes: start..at,
            consumed_end: at,
        });
    }
    Ok(tokens)
}

fn extend_discarded_trivia(tokens: &mut [Token], start: usize, end: usize) {
    if let Some(last) = tokens.last_mut()
        && last.kind.word().is_some()
        && last.consumed_end == start
    {
        last.consumed_end = end;
    }
}

fn kinds(tokens: &[Token]) -> Vec<Kind> {
    tokens.iter().map(|token| token.kind.clone()).collect()
}

/// Include a possible control-word separator, so this bounds both serialized
/// output and the owned strings retained while constructing an expansion.
fn serialized_size(kind: &Kind) -> usize {
    match kind {
        Kind::Word(name) => name.len().saturating_add(2),
        Kind::Symbol(ch) => ch.len_utf8() + 1,
        Kind::Character(ch) => ch.len_utf8(),
        Kind::Open | Kind::Close | Kind::Space | Kind::Paragraph | Kind::Opaque => 1,
    }
}

fn serialized_size_total<'a>(kinds: impl IntoIterator<Item = &'a Kind>) -> usize {
    kinds.into_iter().fold(0usize, |total, kind| {
        total.saturating_add(serialized_size(kind))
    })
}

/// Empty substitutions and unused default arguments can consume work without
/// growing the output. Limit that work separately from the final plan's size.
#[derive(Default)]
struct ExpansionWork {
    tokens: usize,
    bytes: usize,
}

impl ExpansionWork {
    fn charge<'a>(&mut self, kinds: impl IntoIterator<Item = &'a Kind>) -> Result<(), String> {
        for kind in kinds {
            self.tokens = self.tokens.saturating_add(1);
            self.bytes = self.bytes.saturating_add(serialized_size(kind));
            if self.tokens > MAX_TOKENS * 8 || self.bytes > MAX_SOURCE_BYTES * 16 {
                return Err(
                    "These uses need too much expansion work. Inline fewer occurrences at a time."
                        .into(),
                );
            }
        }
        Ok(())
    }
}

/// Serialize precisely the requested tokens. Empty groups and \relax are not
/// separators: either can change argument consumption or math script binding.
fn render(tokens: &[Kind]) -> Result<String, String> {
    let capacity = serialized_size_total(tokens);
    if capacity > MAX_SOURCE_BYTES {
        return Err("The expanded expression exceeds the refactoring size limit.".into());
    }
    let mut output = String::with_capacity(capacity);
    let mut previous: Option<&Kind> = None;
    for kind in tokens {
        if matches!(previous, Some(Kind::Word(_))) {
            if matches!(kind, Kind::Space) {
                return Err("This expansion needs a literal space after a control word; it cannot be inlined without changing TeX tokens.".into());
            }
            if kind.letter() {
                output.push(' ');
            }
        }
        match kind {
            Kind::Word(name) => {
                output.push('\\');
                output.push_str(name);
            }
            Kind::Symbol(ch) => {
                if ch.is_whitespace() {
                    return Err("Explicit control-space tokens need manual refactoring.".into());
                }
                output.push('\\');
                output.push(*ch);
            }
            Kind::Character(ch) => output.push(*ch),
            Kind::Open => output.push('{'),
            Kind::Close => output.push('}'),
            Kind::Space => output.push(' '),
            Kind::Paragraph | Kind::Opaque => {
                return Err(
                    "Paragraph breaks and literal contents cannot be moved into this refactoring."
                        .into(),
                );
            }
        }
        previous = Some(kind);
        if output.len() > MAX_SOURCE_BYTES {
            return Err("The expanded expression exceeds the refactoring size limit.".into());
        }
    }
    Ok(output)
}

#[derive(Debug)]
struct Definition {
    name: String,
    tokens: Range<usize>,
    body: Range<usize>,
    arguments: usize,
    default: Option<Vec<Kind>>,
    global: bool,
    conditional: bool,
    fresh: bool,
}

struct Analysis {
    tokens: Vec<Token>,
    mates: Vec<Option<usize>>,
    optional_closes: Vec<Option<usize>>,
    definitions: Vec<Definition>,
    blocked: Vec<Range<usize>>,
    imports: Vec<usize>,
    document: Option<Range<usize>>,
    environment_boundaries: Vec<usize>,
    comments: Vec<Range<usize>>,
}

impl Analysis {
    fn new(source: &str) -> Result<Self, String> {
        let tokens = lex(source)?;
        let lexical_index = TexDelimiterIndex::new(source);
        let comments = tex_ignored_ranges(source)
            .into_iter()
            .filter(|range| source.as_bytes().get(range.start) == Some(&b'%'))
            .collect();
        let mut mates = vec![None; tokens.len()];
        let mut stack = Vec::new();
        let mut depths = Vec::with_capacity(tokens.len());
        for (index, token) in tokens.iter().enumerate() {
            depths.push(stack.len());
            match token.kind {
                Kind::Open => stack.push(index),
                Kind::Close => {
                    let open = stack
                        .pop()
                        .ok_or("Balance the document's braces before refactoring.")?;
                    mates[open] = Some(index);
                    mates[index] = Some(open);
                }
                _ => {}
            }
        }
        if !stack.is_empty() {
            return Err("Balance the document's braces before refactoring.".into());
        }
        // Optional fields close at the first unbraced ], not at a nested [.
        // Index every suffix once so many overlapping optional readers cannot
        // repeatedly walk the same source or malformed document tail.
        let mut optional_closes = vec![None; tokens.len() + 1];
        for at in (0..tokens.len()).rev() {
            optional_closes[at] = match tokens[at].kind {
                Kind::Open => optional_closes[mates[at].unwrap() + 1],
                Kind::Close | Kind::Paragraph | Kind::Opaque => None,
                Kind::Character(']') => Some(at),
                _ => optional_closes[at + 1],
            };
        }
        let mut result = Self {
            tokens,
            mates,
            optional_closes,
            definitions: Vec::new(),
            blocked: Vec::new(),
            imports: Vec::new(),
            document: None,
            environment_boundaries: Vec::new(),
            comments,
        };
        let mut at = 0;
        let mut environments: Vec<String> = Vec::new();
        let mut document_start = None;
        while at < result.tokens.len() {
            let Some(name) = result.tokens[at].kind.word() else {
                at += 1;
                continue;
            };
            if matches!(
                name,
                "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" | "def"
            ) {
                let definition = result.definition(
                    at,
                    depths[at] == 0
                        && environments.iter().all(|name| name == "document")
                        && !lexical_index.math_at_caret(result.tokens[at].bytes.start),
                )?;
                at = definition.tokens.end;
                result.definitions.push(definition);
                continue;
            }
            if unstable(name) {
                return Err(format!(
                    "\\{name} changes TeX execution or token reading; refactor this document manually."
                ));
            }
            if matches!(
                name,
                "input"
                    | "include"
                    | "subfile"
                    | "usepackage"
                    | "RequirePackage"
                    | "documentclass"
                    | "bibliography"
                    | "tableofcontents"
                    | "listoffigures"
                    | "listoftables"
            ) {
                result.imports.push(at);
            }
            if matches!(name, "begin" | "end") {
                result.environment_boundaries.push(at);
                let argument = result.argument(at + 1)?;
                let environment = result.literal(&argument.inner)?;
                if !known_environment(&environment) {
                    return Err(format!(
                        "The execution behavior of the {environment} environment is unknown."
                    ));
                }
                if name == "begin" {
                    if environment == "document" {
                        if document_start.is_some() {
                            return Err(
                                "Refactoring requires one literal document environment.".into()
                            );
                        }
                        document_start = Some(at..argument.end);
                    }
                    environments.push(environment);
                } else if environments.pop().as_deref() != Some(environment.as_str()) {
                    return Err("Balance the document's environments before refactoring.".into());
                } else if environment == "document" {
                    let open = document_start
                        .clone()
                        .ok_or("Missing document environment.")?;
                    result.document = Some(open.start..argument.end);
                }
            }
            at += 1;
        }
        if !environments.is_empty() {
            return Err("Balance the document's environments before refactoring.".into());
        }
        result.contexts()?;
        result.blocked.sort_by_key(|range| range.start);
        let mut blocked: Vec<Range<usize>> = Vec::new();
        for range in result.blocked.drain(..) {
            if let Some(previous) = blocked.last_mut()
                && range.start <= previous.end
            {
                previous.end = previous.end.max(range.end);
            } else {
                blocked.push(range);
            }
        }
        result.blocked = blocked;
        Ok(result)
    }

    fn literal(&self, range: &Range<usize>) -> Result<String, String> {
        let mut text = String::new();
        for token in &self.tokens[range.clone()] {
            if let Kind::Character(ch) = token.kind {
                text.push(ch);
            } else {
                return Err("A dynamic name cannot be resolved safely.".into());
            }
        }
        Ok(text)
    }

    fn skip_spaces(&self, mut at: usize) -> usize {
        while self.tokens.get(at).is_some_and(|t| t.kind == Kind::Space) {
            at += 1;
        }
        at
    }

    fn argument(&self, at: usize) -> Result<Argument, String> {
        let at = self.skip_spaces(at);
        let token = self
            .tokens
            .get(at)
            .ok_or("Complete the command's arguments before refactoring.")?;
        if token.kind == Kind::Open {
            let close = self.mates[at].ok_or("Unclosed argument.")?;
            Ok(Argument {
                inner: at + 1..close,
                consumed: at..close + 1,
                end: close + 1,
                braced: true,
            })
        } else if matches!(token.kind, Kind::Close | Kind::Paragraph | Kind::Opaque) {
            Err("This argument cannot be resolved safely.".into())
        } else {
            Ok(Argument {
                inner: at..at + 1,
                consumed: at..at + 1,
                end: at + 1,
                braced: false,
            })
        }
    }

    fn optional(&self, at: usize) -> Result<Option<Argument>, String> {
        let start = self.skip_spaces(at);
        if self
            .tokens
            .get(start)
            .is_none_or(|t| t.kind != Kind::Character('['))
        {
            return Ok(None);
        }
        let end = self.optional_closes[start + 1]
            .ok_or("Complete the optional argument before refactoring.")?;
        Ok(Some(Argument {
            inner: start + 1..end,
            consumed: start..end + 1,
            end: end + 1,
            braced: true,
        }))
    }

    fn definition(&self, start: usize, global: bool) -> Result<Definition, String> {
        let command = self.tokens[start].kind.word().unwrap();
        let mut at = start + 1;
        if self
            .tokens
            .get(at)
            .is_some_and(|token| token.kind == Kind::Character('*'))
        {
            at += 1;
        }
        let name = self.argument(at)?;
        if name.inner.len() != 1 {
            return Err("Only literal control-word macro names can be refactored.".into());
        }
        let macro_name = self.tokens[name.inner.start]
            .kind
            .word()
            .ok_or("Only control-word macros can be refactored.")?
            .to_owned();
        at = name.end;
        let mut arguments = 0;
        let mut default = None;
        if command == "def" {
            while self
                .tokens
                .get(at)
                .is_some_and(|t| t.kind == Kind::Character('#'))
            {
                let number = self
                    .tokens
                    .get(at + 1)
                    .and_then(|t| match t.kind {
                        Kind::Character(ch) => ch.to_digit(10),
                        _ => None,
                    })
                    .ok_or("Only consecutive, undelimited \\def parameters are supported.")?
                    as usize;
                if number != arguments + 1 || number > 9 {
                    return Err(
                        "Only consecutive, undelimited \\def parameters are supported.".into(),
                    );
                }
                arguments = number;
                at += 2;
            }
        } else if let Some(count) = self.optional(at)? {
            let text = self.literal(&count.inner)?;
            arguments = text
                .parse::<usize>()
                .ok()
                .filter(|n| *n <= 9)
                .ok_or("The macro needs a literal argument count from 0 to 9.")?;
            at = count.end;
            if let Some(optional) = self.optional(at)? {
                if arguments == 0 {
                    return Err("An optional default requires at least one parameter.".into());
                }
                default = Some(kinds(&self.tokens[optional.inner]));
                at = optional.end;
            }
        }
        let body = self.argument(at)?;
        if !body.braced {
            return Err("The macro definition needs a complete braced body.".into());
        }
        let definition = Definition {
            name: macro_name,
            tokens: start..body.end,
            body: body.inner,
            arguments,
            default,
            global,
            conditional: matches!(command, "providecommand" | "DeclareRobustCommand"),
            fresh: command == "newcommand",
        };
        substitute(
            &kinds(&self.tokens[definition.body.clone()]),
            &vec![Vec::new(); arguments],
        )?;
        if let Some(default) = &definition.default {
            substitute(default, &[])?;
        }
        Ok(definition)
    }

    fn definition_containing(&self, at: usize) -> Option<&Definition> {
        let after = self
            .definitions
            .partition_point(|definition| definition.tokens.start <= at);
        self.definitions
            .get(after.checked_sub(1)?)
            .filter(|definition| definition.tokens.contains(&at))
    }

    fn contexts(&mut self) -> Result<(), String> {
        let mut names = std::collections::BTreeSet::new();
        for definition in &self.definitions {
            if !definition.global
                || definition.conditional
                || !names.insert(definition.name.as_str())
                || signature(&definition.name).is_some()
            {
                return Err("Local scopes, repeated/conditional definitions and overrides of standard commands prevent safe argument analysis.".into());
            }
        }
        let signatures: BTreeMap<_, _> = self
            .definitions
            .iter()
            .map(|d| (d.name.clone(), (d.arguments, d.default.is_some())))
            .collect();
        // A stored macro can execute later, including through another helper.
        // Reject hidden effects in every local body, not just the selected one.
        for definition in &self.definitions {
            let body = self.tokens[definition.body.clone()]
                .iter()
                .map(|token| &token.kind);
            let default = definition.default.iter().flatten();
            for kind in body.chain(default) {
                if let Some(name) = kind.word()
                    && signature(name).is_none()
                    && !signatures.contains_key(name)
                {
                    return Err(format!(
                        "The behavior of \\{name} inside a stored macro is unknown."
                    ));
                }
                if let Kind::Symbol(symbol) = kind
                    && symbol_signature(*symbol).is_none()
                {
                    return Err(format!(
                        "The behavior of \\{symbol} inside a stored macro is unknown."
                    ));
                }
            }
        }
        let mut at = 0;
        while at < self.tokens.len() {
            if let Some(definition) = self.definition_containing(at) {
                at = definition.tokens.end;
                continue;
            }
            if let Kind::Symbol(symbol) = self.tokens[at].kind {
                let (count, optional, transparent) = symbol_signature(symbol)
                    .ok_or_else(|| format!("The argument behavior of \\{symbol} is unknown."))?;
                self.context_arguments(at, count, optional, transparent, false, symbol == '\\')?;
                at += 1;
                continue;
            }
            let Some(name) = self.tokens[at].kind.word().map(str::to_owned) else {
                at += 1;
                continue;
            };
            let local = signatures.contains_key(&name);
            let (count, optional, transparent) = if let Some((count, optional)) =
                signatures.get(&name)
            {
                (*count, *optional, false)
            } else if let Some(signature) = signature(&name) {
                signature
            } else {
                return Err(format!(
                    "The argument behavior of \\{name} is unknown; a safe refactoring is unavailable here."
                ));
            };
            if matches!(name.as_str(), "dots" | "newline") {
                let next = self.skip_spaces(at + 1);
                if next < self.tokens.len() {
                    self.blocked.push(next..next + 1);
                }
            }
            self.context_arguments(
                at,
                count,
                optional,
                transparent,
                local,
                !local
                    && matches!(
                        name.as_str(),
                        "section"
                            | "subsection"
                            | "subsubsection"
                            | "chapter"
                            | "part"
                            | "paragraph"
                            | "subparagraph"
                            | "operatorname"
                    ),
            )?;
            at += 1;
        }
        Ok(())
    }

    fn context_arguments(
        &mut self,
        at: usize,
        count: usize,
        optional: bool,
        transparent: bool,
        local_optional: bool,
        starred: bool,
    ) -> Result<(), String> {
        let mut next = at + 1;
        if starred
            && self
                .tokens
                .get(next)
                .is_some_and(|token| token.kind == Kind::Character('*'))
        {
            next += 1;
        }
        let mut required = count;
        if optional {
            if let Some(argument) = self.optional(next)? {
                // Optional argument delimiters are scanned before their
                // contents expand. A macro can hide a literal closing ],
                // so even a normal renderer's optional field is opaque.
                self.blocked.push(argument.consumed.clone());
                next = argument.end;
            } else {
                // Replacing the next raw token could manufacture a new
                // optional [field] that was not present before expansion.
                let next = self.skip_spaces(next);
                if next < self.tokens.len() {
                    self.blocked.push(next..next + 1);
                }
            }
            if local_optional {
                required = required.saturating_sub(1);
            }
        }
        for _ in 0..required {
            let argument = self.argument(next)?;
            if !transparent || !argument.braced {
                self.blocked.push(argument.consumed.clone());
            }
            next = argument.end;
        }
        Ok(())
    }

    fn admits(&self, range: &Range<usize>) -> Result<(), String> {
        let definition = self
            .definitions
            .partition_point(|definition| definition.tokens.end <= range.start);
        if self
            .definitions
            .get(definition)
            .is_some_and(|definition| overlaps(&definition.tokens, range))
        {
            return Err("Macro definitions are stored tokens, not direct uses.".into());
        }
        let blocked = self
            .blocked
            .partition_point(|blocked| blocked.end <= range.start);
        if self
            .blocked
            .get(blocked)
            .is_some_and(|blocked| overlaps(blocked, range))
        {
            return Err("This use is inside a token-reading or unbraced command argument. Refactor the surrounding expression manually.".into());
        }
        Ok(())
    }

    fn contains_comment(&self, bytes: &Range<usize>) -> bool {
        let at = self
            .comments
            .partition_point(|comment| comment.end <= bytes.start);
        self.comments
            .get(at)
            .is_some_and(|comment| overlaps(comment, bytes))
    }
}

struct Argument {
    inner: Range<usize>,
    consumed: Range<usize>,
    end: usize,
    braced: bool,
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

fn known_environment(name: &str) -> bool {
    matches!(
        name,
        "document"
            | "math"
            | "displaymath"
            | "equation"
            | "equation*"
            | "align"
            | "align*"
            | "aligned"
            | "gather"
            | "gather*"
            | "gathered"
            | "multline"
            | "multline*"
            | "split"
            | "matrix"
            | "pmatrix"
            | "bmatrix"
            | "Bmatrix"
            | "vmatrix"
            | "Vmatrix"
            | "smallmatrix"
            | "cases"
            | "center"
            | "flushleft"
            | "flushright"
            | "itemize"
            | "enumerate"
            | "description"
            | "abstract"
            | "quote"
            | "quotation"
            | "verse"
    )
}

fn unstable(name: &str) -> bool {
    name.starts_with("if")
        || matches!(
            name,
            "else"
                | "fi"
                | "catcode"
                | "mathcode"
                | "delcode"
                | "lccode"
                | "uccode"
                | "sfcode"
                | "endlinechar"
                | "escapechar"
                | "newlinechar"
                | "uppercase"
                | "lowercase"
                | "obeyspaces"
                | "obeylines"
                | "makeatletter"
                | "makeatother"
                | "ExplSyntaxOn"
                | "ExplSyntaxOff"
                | "let"
                | "futurelet"
                | "global"
                | "long"
                | "outer"
                | "protected"
                | "gdef"
                | "edef"
                | "xdef"
                | "csname"
                | "endcsname"
                | "expandafter"
                | "noexpand"
                | "string"
                | "meaning"
                | "scantokens"
                | "everymath"
                | "everydisplay"
                | "everypar"
                | "newenvironment"
                | "renewenvironment"
                | "NewDocumentCommand"
                | "RenewDocumentCommand"
                | "ProvideDocumentCommand"
                | "DeclareDocumentCommand"
        )
}

/// Known direct renderers expand within braced fields. Everything else has a
/// fixed opaque argument signature or is a standard zero-argument math token.
fn signature(name: &str) -> Option<(usize, bool, bool)> {
    let signature = match name {
        "frac" | "dfrac" | "tfrac" | "binom" | "dbinom" | "tbinom" | "overset" | "underset"
        | "stackrel" => (2, false, true),
        "sqrt" | "smash" => (1, true, true),
        "pmod" | "mod" => (1, false, true),
        "item" | "linebreak" | "pagebreak" => (0, true, false),
        // Accents can inspect a field before its macro tokens expand. The
        // pinned compiler proved that even a braced text-accent argument is
        // opaque; math/decorative accent wrappers follow the same safe rule.
        "overline" | "underline" | "overbrace" | "underbrace" | "hat" | "widehat" | "tilde"
        | "widetilde" | "bar" | "vec" | "dot" | "ddot" | "dddot" | "ddddot" | "breve" | "check"
        | "acute" | "grave" => (1, false, false),
        "text" | "textrm" | "textsf" | "texttt" | "textbf" | "textit" | "textsl" | "textsc"
        | "textnormal" | "emph" | "mbox" | "hbox" | "mathrm" | "mathsf" | "mathtt" | "mathbf"
        | "mathit" | "mathnormal" | "mathcal" | "mathbb" | "mathfrak" | "mathscr"
        | "boldsymbol" | "bm" | "operatorname" | "mathop" | "mathord" | "mathrel" | "mathbin"
        | "mathopen" | "mathclose" | "mathpunct" | "mathinner" | "phantom" | "hphantom"
        | "vphantom" => (1, false, true),
        "documentclass" | "usepackage" | "RequirePackage" | "includegraphics" | "section"
        | "subsection" | "subsubsection" | "chapter" | "part" | "paragraph" | "subparagraph"
        | "cite" | "citep" | "citet" | "parencite" | "textcite" | "autocite" | "addbibresource" => {
            (1, true, false)
        }
        "begin" | "end" | "label" | "ref" | "eqref" | "pageref" | "input" | "include"
        | "subfile" | "bibliography" | "bibliographystyle" | "title" | "author" | "date"
        | "caption" | "url" | "path" | "nolinkurl" | "detokenize" | "ensuremath" | "footnote"
        | "thanks" | "index" | "glossary" => (1, false, false),
        "href" => (2, false, false),
        "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr"
        | "biggl" | "biggr" | "Biggl" | "Biggr" => (1, false, false),
        _ if ZERO_ARGUMENT_NAMES.contains(name) => (0, false, true),
        _ => return None,
    };
    Some(signature)
}

const ZERO_ARGUMENT: &str = "alpha beta gamma delta epsilon varepsilon zeta eta theta vartheta iota kappa varkappa lambda mu nu xi omicron pi varpi rho varrho sigma varsigma tau upsilon phi varphi chi psi omega Gamma Delta Theta Lambda Xi Pi Sigma Upsilon Phi Psi Omega partial nabla ell imath jmath hbar Re Im wp infty emptyset varnothing aleph beth gimel daleth prime backprime forall exists nexists neg lnot top bot land lor wedge vee cap cup bigcap bigcup uplus sqcap sqcup setminus smallsetminus times cdot div pm mp ast star circ bullet oplus ominus otimes oslash odot bigoplus bigotimes bigodot sum prod coprod int iint iiint iiiint oint lim limsup liminf inf sup max min log ln exp sin cos tan cot sec csc arcsin arccos arctan sinh cosh tanh det dim ker hom gcd Pr mod bmod pmod quad qquad thinspace medspace thickspace enspace in notin ni owns subset supset subseteq supseteq subsetneq supsetneq sqsubset sqsupset sqsubseteq sqsupseteq le leq leqslant ge geq geqslant ne neq equiv approx sim simeq cong propto asymp prec succ preceq succeq ll gg perp parallel mid nmid models vdash dashv to mapsto gets rightarrow leftarrow leftrightarrow Rightarrow Leftarrow Leftrightarrow longrightarrow longleftarrow longleftrightarrow Longrightarrow Longleftarrow Longleftrightarrow hookrightarrow hookleftarrow uparrow downarrow updownarrow Uparrow Downarrow Updownarrow nearrow searrow swarrow nwarrow langle rangle lfloor rfloor lceil rceil lbrace rbrace vert Vert lvert rvert lVert rVert backslash angle triangle square diamond Box Diamond checkmark clubsuit diamondsuit heartsuit spadesuit dots ldots cdots vdots ddots displaystyle textstyle scriptstyle scriptscriptstyle limits nolimits nonumber notag hfill vfill newline linebreak pagebreak noindent indent par medskip smallskip bigskip maketitle tableofcontents listoffigures listoftables clearpage newpage centering raggedright raggedleft today LaTeX TeX textwidth linewidth columnwidth baselineskip strut relax normalfont rmfamily sffamily ttfamily bfseries mdseries itshape upshape slshape scshape tiny scriptsize footnotesize small normalsize large Large LARGE huge Huge item";

static ZERO_ARGUMENT_NAMES: std::sync::LazyLock<std::collections::HashSet<&'static str>> =
    std::sync::LazyLock::new(|| ZERO_ARGUMENT.split_ascii_whitespace().collect());

fn symbol_signature(symbol: char) -> Option<(usize, bool, bool)> {
    match symbol {
        '\'' | '"' | '`' | '^' | '~' | '=' | '.' => Some((1, false, false)),
        '\\' => Some((0, true, false)),
        '%' | '$' | '#' | '&' | '_' | '{' | '}' | '(' | ')' | '[' | ']' | '!' | ',' | ':' | ';'
        | '>' | '|' | '/' | '-' => Some((0, false, true)),
        _ => None,
    }
}

fn substitute(body: &[Kind], arguments: &[Vec<Kind>]) -> Result<Vec<Kind>, String> {
    let mut output = Vec::new();
    let argument_bytes = arguments
        .iter()
        .map(serialized_size_total)
        .collect::<Vec<_>>();
    let mut output_bytes = 0usize;
    let mut at = 0;
    while at < body.len() {
        if body[at] == Kind::Character('#') {
            let Some(Kind::Character(number)) = body.get(at + 1) else {
                return Err(
                    "Literal or nested parameter markers require manual refactoring.".into(),
                );
            };
            let index = number
                .to_digit(10)
                .filter(|n| *n > 0)
                .ok_or("Invalid macro parameter marker.")? as usize
                - 1;
            let argument = arguments
                .get(index)
                .ok_or("The macro body references an undeclared parameter.")?;
            if output.len().saturating_add(argument.len()) > MAX_TOKENS {
                return Err("The expanded expression exceeds the refactoring token limit.".into());
            }
            output_bytes = output_bytes.saturating_add(argument_bytes[index]);
            if output_bytes > MAX_SOURCE_BYTES {
                return Err("The expanded expression exceeds the refactoring size limit.".into());
            }
            output.extend(argument.iter().cloned());
            at += 2;
        } else {
            if matches!(&body[at], Kind::Word(name) if unstable(name) || matches!(name.as_str(), "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" | "def" | "detokenize" | "input" | "include" | "subfile" | "usepackage" | "RequirePackage" | "documentclass" | "begin" | "end" | "bibliography" | "tableofcontents" | "listoffigures" | "listoftables"))
            {
                return Err(
                    "The macro body changes TeX token reading or contains a nested definition."
                        .into(),
                );
            }
            output_bytes = output_bytes.saturating_add(serialized_size(&body[at]));
            if output.len() >= MAX_TOKENS {
                return Err("The expanded expression exceeds the refactoring token limit.".into());
            }
            if output_bytes > MAX_SOURCE_BYTES {
                return Err("The expanded expression exceeds the refactoring size limit.".into());
            }
            output.push(body[at].clone());
            at += 1;
        }
    }
    Ok(output)
}

struct Expansion {
    tokens: Range<usize>,
    bytes: Range<usize>,
    body: Vec<Kind>,
}

fn expansion(
    analysis: &Analysis,
    definition: &Definition,
    at: usize,
    work: &mut ExpansionWork,
) -> Result<Expansion, String> {
    if !definition.global || definition.conditional {
        return Err(
            "Only unconditional, unprotected macros defined at document scope can be inlined."
                .into(),
        );
    }
    if at < definition.tokens.end {
        return Err("This use precedes the local macro definition.".into());
    }
    if !definition.fresh {
        let boundary = analysis
            .environment_boundaries
            .partition_point(|position| *position < definition.tokens.end);
        if analysis
            .environment_boundaries
            .get(boundary)
            .is_some_and(|position| *position < at)
        {
            return Err("An environment or document hook could replace this existing command. Only fresh \\newcommand definitions can cross environment boundaries.".into());
        }
    }
    let after_definition = analysis
        .imports
        .partition_point(|position| *position < definition.tokens.end);
    if analysis
        .imports
        .get(after_definition)
        .is_some_and(|position| *position < at)
    {
        return Err("An included file or package could redefine this macro before its use.".into());
    }
    analysis.admits(&(at..at + 1))?;
    work.charge(
        analysis.tokens[definition.body.clone()]
            .iter()
            .map(|token| &token.kind),
    )?;
    let mut next = at + 1;
    let mut arguments = Vec::new();
    if let Some(default) = &definition.default {
        if let Some(argument) = analysis.optional(next)? {
            work.charge(
                analysis.tokens[argument.inner.clone()]
                    .iter()
                    .map(|token| &token.kind),
            )?;
            arguments.push(kinds(&analysis.tokens[argument.inner]));
            next = argument.end;
        } else {
            work.charge(default)?;
            arguments.push(default.clone());
            next = analysis.skip_spaces(next);
        }
    }
    while arguments.len() < definition.arguments {
        let argument = analysis.argument(next)?;
        work.charge(
            analysis.tokens[argument.inner.clone()]
                .iter()
                .map(|token| &token.kind),
        )?;
        arguments.push(kinds(&analysis.tokens[argument.inner]));
        next = argument.end;
    }
    let range = at..next;
    let body = substitute(
        &kinds(&analysis.tokens[definition.body.clone()]),
        &arguments,
    )?;
    if body
        .iter()
        .any(|kind| kind.word() == Some(definition.name.as_str()))
    {
        return Err("Recursive macro expansion requires manual refactoring.".into());
    }
    let bytes = analysis.tokens[at].bytes.start..analysis.tokens[next - 1].consumed_end;
    if analysis.contains_comment(&(bytes.start..analysis.tokens[next - 1].bytes.end)) {
        return Err("Inlining would move or remove a comment inside this invocation. Refactor a smaller expression manually.".into());
    }
    Ok(Expansion {
        tokens: range,
        bytes,
        body,
    })
}

pub(crate) fn inline(source: &str, caret_byte: usize, scope: InlineScope) -> Result<Plan, String> {
    if caret_byte > source.len() || !source.is_char_boundary(caret_byte) {
        return Err("The cursor is not at a valid TeX token boundary.".into());
    }
    let analysis = Analysis::new(source)?;
    let selected = analysis
        .tokens
        .iter()
        .position(|token| token.kind.word().is_some() && token.bytes.contains(&caret_byte))
        .or_else(|| {
            analysis
                .tokens
                .iter()
                .position(|token| token.kind.word().is_some() && token.bytes.end == caret_byte)
        })
        .ok_or("Place the cursor on a locally defined macro call.")?;
    let name = analysis.tokens[selected].kind.word().unwrap();
    if signature(name).is_some() {
        return Err("An overridden standard command can change inside environments; only new local command names are supported.".into());
    }
    let definitions: Vec<_> = analysis
        .definitions
        .iter()
        .filter(|d| d.name == name)
        .collect();
    let [definition] = definitions.as_slice() else {
        return Err(if definitions.is_empty() {
            "No literal local definition was found for this macro."
        } else {
            "This macro is redefined or shadowed; its meaning is ambiguous."
        }
        .into());
    };
    let positions: Vec<_> = match scope {
        InlineScope::Occurrence => vec![selected],
        InlineScope::AllOccurrences => analysis
            .tokens
            .iter()
            .enumerate()
            .filter_map(|(at, token)| {
                (token.kind.word() == Some(name) && analysis.definition_containing(at).is_none())
                    .then_some(at)
            })
            .collect(),
    };
    if positions.is_empty() {
        return Err("There are no direct uses of this macro to inline.".into());
    }
    let occurrences = positions.len();
    let original_tokens = kinds(&analysis.tokens);
    let mut replacement = String::with_capacity(source.len());
    let mut expected = Vec::with_capacity(original_tokens.len());
    let mut selection = None;
    let mut source_at = 0;
    let mut token_at = 0;
    let mut expanded_tokens = 0usize;
    let mut expanded_bytes = 0usize;
    let mut work = ExpansionWork::default();
    // Build once in source order and release each candidate after moving its
    // tokens into verification. Do not retain one allocated body per call.
    for at in positions {
        let expansion = expansion(&analysis, definition, at, &mut work)?;
        if expansion.tokens.start < token_at || expansion.bytes.start < source_at {
            return Err("Nested uses overlap. Inline the inner use first.".into());
        }
        expanded_tokens = expanded_tokens.saturating_add(expansion.body.len());
        if expanded_tokens > MAX_TOKENS {
            return Err("All uses together exceed the refactoring token limit.".into());
        }
        expanded_bytes = expanded_bytes.saturating_add(serialized_size_total(&expansion.body));
        if expanded_bytes > MAX_SOURCE_BYTES {
            return Err("All uses together exceed the refactoring size limit.".into());
        }
        if expected
            .len()
            .saturating_add(expansion.tokens.start - token_at)
            .saturating_add(expansion.body.len())
            > MAX_TOKENS
        {
            return Err("The refactored document exceeds the refactoring token limit.".into());
        }
        expected.extend_from_slice(&original_tokens[token_at..expansion.tokens.start]);
        let mut inserted = render(&expansion.body)?;
        if expansion.body.first().is_some_and(Kind::letter)
            && expected.last().is_some_and(|kind| kind.word().is_some())
        {
            inserted.insert(0, ' ');
        }
        if matches!(expansion.body.last(), Some(Kind::Word(_)))
            && analysis
                .tokens
                .get(expansion.tokens.end)
                .is_some_and(|token| token.kind.letter())
        {
            inserted.push(' ');
        }
        let selection_length = inserted.len();
        let tail =
            &source[analysis.tokens[expansion.tokens.end - 1].bytes.end..expansion.bytes.end];
        if tail.contains('%') {
            // Retain caller comments and suppression markers in their exact
            // order. If this introduces a visible space, the final whole-file
            // lexical comparison refuses the edit rather than dropping them.
            inserted.push_str(tail);
        }
        if replacement
            .len()
            .saturating_add(expansion.bytes.start - source_at)
            .saturating_add(inserted.len())
            > MAX_SOURCE_BYTES
        {
            return Err("The refactored document exceeds the 4 MiB size limit.".into());
        }
        replacement.push_str(&source[source_at..expansion.bytes.start]);
        let insertion_start = replacement.len();
        replacement.push_str(&inserted);
        if selection.is_none() || expansion.tokens.start == selected {
            selection = Some(insertion_start..insertion_start + selection_length);
        }
        source_at = expansion.bytes.end;
        token_at = expansion.tokens.end;
        expected.extend(expansion.body);
    }
    if expected
        .len()
        .saturating_add(original_tokens.len() - token_at)
        > MAX_TOKENS
    {
        return Err("The refactored document exceeds the refactoring token limit.".into());
    }
    if replacement.len().saturating_add(source.len() - source_at) > MAX_SOURCE_BYTES {
        return Err("The refactored document exceeds the 4 MiB size limit.".into());
    }
    replacement.push_str(&source[source_at..]);
    expected.extend_from_slice(&original_tokens[token_at..]);
    if kinds(&lex(&replacement)?) != expected {
        return Err("Inlining would change whitespace, token boundaries, or script parsing. Refactor this use manually.".into());
    }
    Ok(Plan {
        original: source.into(),
        replacement,
        selection: selection.unwrap(),
        label: format!("Inline \\{name}"),
        occurrences,
    })
}

pub(crate) fn extract(
    source: &str,
    selection_bytes: Range<usize>,
    name: &str,
) -> Result<Plan, String> {
    let name = name.strip_prefix('\\').unwrap_or(name);
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err("Use a command name containing ASCII letters only.".into());
    }
    if selection_bytes.is_empty() || source.get(selection_bytes.clone()).is_none() {
        return Err("Select a complete expression to extract.".into());
    }
    let analysis = Analysis::new(source)?;
    if analysis.contains_comment(&selection_bytes) {
        return Err("Extraction would move or remove a comment or suppression marker. Select the expression without comments.".into());
    }
    if signature(name).is_some()
        || analysis
            .tokens
            .iter()
            .any(|token| token.kind.word() == Some(name))
    {
        return Err("That command name is already used. Choose a new name.".into());
    }
    let document = analysis.document.as_ref().ok_or(
        "Extraction requires a complete literal \\begin{document} ... \\end{document} file.",
    )?;
    let start = analysis
        .tokens
        .partition_point(|token| token.bytes.end <= selection_bytes.start);
    let mut end = analysis
        .tokens
        .partition_point(|token| token.bytes.start < selection_bytes.end);
    if start == end
        || analysis.tokens[start].bytes.start < selection_bytes.start
        || analysis.tokens[end - 1].bytes.end > selection_bytes.end
    {
        return Err("The selection splits a TeX command or token.".into());
    }
    if start <= document.start || end >= document.end {
        return Err("Select an expression inside the document body.".into());
    }
    analysis.admits(&(start..end))?;
    for at in start..end {
        if let Some(mate) = analysis.mates[at]
            && !(start..end).contains(&mate)
        {
            return Err("The selection crosses a brace group boundary.".into());
        }
        if matches!(
            analysis.tokens[at].kind,
            Kind::Character('$' | '#') | Kind::Symbol('(' | ')' | '[' | ']')
        ) {
            return Err(
                "Select the expression inside its math delimiters, without parameter markers."
                    .into(),
            );
        }
        if matches!(&analysis.tokens[at].kind, Kind::Word(name) if matches!(name.as_str(), "begin" | "end"))
        {
            return Err("Environment boundaries cannot be extracted into a command.".into());
        }
    }
    // A newly inserted control word would swallow a following real space.
    // Move that token into its body rather than inserting an empty group.
    let mut replaced = selection_bytes.clone();
    if analysis
        .tokens
        .get(end)
        .is_some_and(|token| token.kind == Kind::Space && token.bytes.start == replaced.end)
    {
        replaced.end = analysis.tokens[end].bytes.end;
        end += 1;
    }
    let body = kinds(&analysis.tokens[start..end]);
    let rendered = render(&body)?;
    let mut invocation = format!("\\{name}");
    if analysis
        .tokens
        .get(end)
        .is_some_and(|token| token.kind.letter())
    {
        invocation.push(' ');
    }
    let mut replacement = source.to_owned();
    replacement.replace_range(replaced.clone(), &invocation);
    let insertion = analysis.tokens[document.start].bytes.start;
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let declaration = format!("\\newcommand{{\\{name}}}{{{rendered}}}%{newline}");
    replacement.insert_str(insertion, &declaration);
    let selection_start = replaced.start + declaration.len();
    let plan = Plan {
        original: source.into(),
        replacement,
        selection: selection_start..selection_start + invocation.len(),
        label: format!("Extract \\{name}"),
        occurrences: 1,
    };
    // Round-trip the new command in the generated document. Comparing tokens
    // ignores harmless preamble declaration layout, but proves the original
    // body token stream returns exactly, without added grouping or lost spaces.
    let roundtrip = inline(plan.preview(), selection_start, InlineScope::Occurrence)?;
    let mut expected = source.to_owned();
    expected.insert_str(insertion, &declaration);
    if kinds(&lex(roundtrip.preview())?) != kinds(&lex(&expected)?) {
        return Err("Extraction would change the document's TeX tokens.".into());
    }
    // Build the lexical index only on this explicit action, never each frame.
    // It additionally catches selections crossing an inline/display boundary.
    let index = TexDelimiterIndex::new(source);
    if index.math_at_caret(selection_bytes.start)
        != index.math_at_caret(selection_bytes.end.saturating_sub(1))
    {
        return Err("The selection crosses a math boundary.".into());
    }
    Ok(plan)
}

#[cfg(test)]
mod perf_tests;
#[cfg(test)]
mod tests;
