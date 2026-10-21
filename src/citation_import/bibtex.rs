use super::{MAX_BIB_BYTES, MAX_ENTRY_BYTES, identifier};
use std::{collections::BTreeMap, ops::Range};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) bibtex: String,
    pub(super) fields: BTreeMap<String, Option<String>>,
    key_range: Range<usize>,
}

impl Entry {
    pub(crate) fn doi(&self) -> Option<String> {
        let value = self.fields.get("doi")?.as_ref()?;
        match identifier::Identifier::parse(&identifier_literal(value)).ok()? {
            identifier::Identifier::Doi(value) => Some(value),
            _ => None,
        }
    }

    pub(super) fn arxiv(&self) -> Option<String> {
        for field in ["eprint", "arxiv", "url"] {
            if let Some(Some(value)) = self.fields.get(field)
                && let Some(id) = identifier::arxiv_identity(&identifier_literal(value))
            {
                return Some(id);
            }
        }
        self.doi().and_then(|doi| {
            doi.strip_prefix("10.48550/arxiv.")
                .and_then(identifier::arxiv_identity)
        })
    }
}

fn identifier_literal(value: &str) -> String {
    let mut value = value.trim();
    while let Some(inner) = value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
    {
        let mut depth = 0_isize;
        if inner.chars().any(|character| {
            if character == '{' {
                depth += 1;
            }
            if character == '}' {
                depth -= 1;
            }
            depth < 0
        }) || depth != 0
        {
            break;
        }
        value = inner.trim();
    }
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\'
            && characters
                .peek()
                .is_some_and(|character| matches!(character, '_' | '%' | '&' | '#' | '{' | '}'))
        {
            output.push(characters.next().unwrap());
        } else {
            output.push(character);
        }
    }
    output
}

pub(crate) fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b':' | b'.' | b'+'))
    {
        return Err("Use a citation key of 1–128 letters, numbers, _, -, :, ., or +.".into());
    }
    Ok(())
}

pub(crate) fn parse_entry(source: &str) -> Result<Entry, String> {
    if source.len() > MAX_ENTRY_BYTES {
        return Err("The citation entry is too large.".into());
    }
    let mut entries = parse(source, true)?;
    if entries.len() != 1 {
        return Err("Review exactly one BibTeX entry.".into());
    }
    let entry = entries.remove(0);
    validate_key(&entry.key)?;
    Ok(entry)
}

pub(crate) fn rekey(source: &str, key: &str) -> Result<String, String> {
    validate_key(key)?;
    let entry = parse_entry(source)?;
    let mut output = entry.bibtex.clone();
    output.replace_range(entry.key_range, key);
    Ok(output)
}

pub(super) fn parse_database(source: &str) -> Result<Vec<Entry>, String> {
    parse(source, false)
}

fn parse(source: &str, standalone: bool) -> Result<Vec<Entry>, String> {
    if source.len() > MAX_BIB_BYTES {
        return Err("The bibliography exceeds the 16 MiB limit.".into());
    }
    if source.contains('\0') {
        return Err("The bibliography contains a null character.".into());
    }
    let mut parser = Parser {
        source,
        cursor: usize::from(source.starts_with('\u{feff}')) * 3,
        macros: BTreeMap::new(),
        standalone,
    };
    let mut entries = Vec::new();
    while parser.skip() {
        let start = parser.cursor;
        parser.require(b'@')?;
        let kind = parser.name()?.to_ascii_lowercase();
        parser.skip();
        let closing = match parser.next() {
            Some(b'{') => b'}',
            Some(b'(') => b')',
            _ => return parser.error(),
        };
        if kind == "comment" {
            parser.comment(closing)?;
            continue;
        }
        if kind == "preamble" {
            if standalone {
                return Err("Review one standalone entry, without a BibTeX preamble.".into());
            }
            parser.value()?;
            parser.skip();
            parser.eat(b',');
            parser.skip();
            parser.require(closing)?;
            continue;
        }
        parser.skip();
        if kind == "string" {
            if standalone {
                return Err(
                    "Review one standalone entry, without external BibTeX string definitions."
                        .into(),
                );
            }
            let name = parser.name()?.to_ascii_lowercase();
            parser.skip();
            parser.require(b'=')?;
            parser.skip();
            let value = parser.value()?;
            parser.skip();
            parser.eat(b',');
            parser.skip();
            parser.require(closing)?;
            parser.macros.insert(name, value);
            continue;
        }
        let key_start = parser.cursor;
        while parser.peek().is_some_and(|c| c != b',' && c != closing) {
            parser.cursor += 1;
        }
        let key_end = parser.cursor;
        let key_text = source[key_start..key_end].trim();
        if key_text.is_empty()
            || key_text.chars().any(|c| {
                c.is_whitespace()
                    || matches!(c, '{' | '}' | '(' | ')' | '"' | '%' | '@' | '=' | '#')
            })
        {
            return Err("A bibliography entry has an invalid citation key.".into());
        }
        let key_offset = source[key_start..key_end].find(key_text).unwrap();
        let mut fields = BTreeMap::new();
        if !parser.eat(closing) {
            parser.require(b',')?;
            loop {
                parser.skip();
                if parser.eat(closing) {
                    break;
                }
                let field = parser.name()?.to_ascii_lowercase();
                parser.skip();
                parser.require(b'=')?;
                parser.skip();
                let value = parser.value()?;
                if fields.insert(field, value).is_some() {
                    return Err("A BibTeX entry repeats a field name.".into());
                }
                parser.skip();
                if parser.eat(closing) {
                    break;
                }
                parser.require(b',')?;
            }
        }
        if parser.cursor - start > MAX_ENTRY_BYTES {
            return Err("A bibliography entry is too large.".into());
        }
        entries.push(Entry {
            key: key_text.to_owned(),
            bibtex: source[start..parser.cursor].to_owned(),
            fields,
            key_range: (key_start + key_offset - start)
                ..(key_start + key_offset + key_text.len() - start),
        });
    }
    Ok(entries)
}

struct Parser<'a> {
    source: &'a str,
    cursor: usize,
    macros: BTreeMap<String, Option<String>>,
    standalone: bool,
}
impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.cursor).copied()
    }
    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.cursor += 1;
        Some(byte)
    }
    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn require(&mut self, byte: u8) -> Result<(), String> {
        if self.eat(byte) { Ok(()) } else { self.error() }
    }
    fn error<T>(&self) -> Result<T, String> {
        Err("The bibliography contains invalid or incomplete BibTeX. No file was changed.".into())
    }
    fn skip(&mut self) -> bool {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.cursor += 1;
            }
            if self.eat(b'%') {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.cursor += 1;
                }
            } else {
                return self.peek().is_some();
            }
        }
    }
    fn name(&mut self) -> Result<&str, String> {
        let start = self.cursor;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b':' | b'.'))
        {
            self.cursor += 1;
        }
        if self.cursor == start {
            self.error()
        } else {
            Ok(&self.source[start..self.cursor])
        }
    }
    fn comment(&mut self, closing: u8) -> Result<(), String> {
        let opening = if closing == b'}' { b'{' } else { b'(' };
        let mut depth = 1;
        while let Some(byte) = self.next() {
            if byte == b'\\' {
                self.next();
                continue;
            }
            if byte == opening {
                depth += 1;
                if depth > 64 {
                    return self.error();
                }
            }
            if byte == closing {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
        }
        self.error()
    }
    fn atom(&mut self) -> Result<Option<String>, String> {
        let quote = self.peek() == Some(b'"');
        if quote || self.peek() == Some(b'{') {
            self.cursor += 1;
            let start = self.cursor;
            let mut depth = usize::from(!quote);
            while let Some(byte) = self.next() {
                if byte == b'\\' {
                    self.next();
                    continue;
                }
                if byte == b'{' {
                    depth += 1;
                    if depth > 64 {
                        return self.error();
                    }
                } else if byte == b'}' {
                    if depth == 0 {
                        return self.error();
                    }
                    depth -= 1;
                    if !quote && depth == 0 {
                        return Ok(Some(self.source[start..self.cursor - 1].to_owned()));
                    }
                } else if byte == b'"' && quote && depth == 0 {
                    return Ok(Some(self.source[start..self.cursor - 1].to_owned()));
                }
            }
            return self.error();
        }
        let name = self.name()?.to_owned();
        if name.bytes().all(|c| c.is_ascii_digit()) {
            return Ok(Some(name));
        }
        let normalized = name.to_ascii_lowercase();
        if let Some(value) = self.macros.get(&normalized) {
            return Ok(value.clone());
        }
        if [
            "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
        ]
        .contains(&normalized.as_str())
        {
            return Ok(Some(normalized));
        }
        if self.standalone {
            return Err("Use literal field values in the reviewed entry, rather than an undefined BibTeX string.".into());
        }
        Ok(None)
    }
    fn value(&mut self) -> Result<Option<String>, String> {
        let mut value = self.atom()?;
        loop {
            self.skip();
            if !self.eat(b'#') {
                break;
            }
            self.skip();
            value = value.zip(self.atom()?).map(|(left, right)| left + &right);
        }
        Ok(value)
    }
}
