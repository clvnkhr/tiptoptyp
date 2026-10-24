//! Literal TeX structure, without executing macros or searching installed packages.
use super::*;

pub(super) fn visit(
    path: &Path,
    source: &str,
    index: &mut ProjectIndex,
    packages: &mut BTreeSet<String>,
    mut local_file: impl FnMut(&str, bool) -> bool,
    cancelled: &mut impl FnMut() -> bool,
) -> bool {
    let lines = LineIndex::new(source);
    let mut scan = Scanner {
        source,
        at: 0,
        checked: 0,
        aborted: false,
        cancelled,
    };
    while scan.at < source.len() && scan.check() {
        match source.as_bytes()[scan.at] {
            b'%' => scan.comment(),
            b'\\' => {
                let start = scan.at;
                let name = scan.command();
                if matches!(name, "verb" | "Verb" | "lstinline" | "mintinline") {
                    scan.inline_literal(name);
                    continue;
                }
                if name == "string" {
                    if scan.source.as_bytes().get(scan.at) == Some(&b'\\') {
                        scan.command();
                    } else {
                        scan.character();
                    }
                    continue;
                }
                if name == "detokenize" {
                    scan.group(b'{');
                    continue;
                }
                scan.star();
                if name == "begin" {
                    if let Some(environment) = scan.group(b'{')
                        && matches!(
                            environment.trim(),
                            "verbatim"
                                | "verbatim*"
                                | "Verbatim"
                                | "Verbatim*"
                                | "lstlisting"
                                | "minted"
                                | "comment"
                                | "filecontents"
                                | "filecontents*"
                        )
                    {
                        let closing = format!("\\end{{{}}}", environment.trim());
                        if environment.trim() == "comment" {
                            scan.comment_environment(&closing);
                        } else {
                            scan.at = source[scan.at..]
                                .find(&closing)
                                .map_or(source.len(), |n| scan.at + n + closing.len());
                        }
                    }
                    continue;
                }
                if matches!(
                    name,
                    "newcommand"
                        | "renewcommand"
                        | "providecommand"
                        | "DeclareRobustCommand"
                        | "DeclareMathOperator"
                        | "NewDocumentCommand"
                        | "RenewDocumentCommand"
                        | "ProvideDocumentCommand"
                        | "DeclareDocumentCommand"
                        | "def"
                        | "gdef"
                        | "edef"
                        | "xdef"
                ) {
                    let macro_name = scan.macro_name();
                    if let Some(macro_name) = macro_name {
                        index.symbols.push(SymbolEntry {
                            path: path.into(),
                            line: lines.line_at(start),
                            name: macro_name,
                            kind: SymbolKind::Function,
                        });
                    }
                    if matches!(name, "def" | "gdef" | "edef" | "xdef") {
                        while scan.at < source.len()
                            && source.as_bytes()[scan.at] != b'{'
                            && scan.check()
                        {
                            scan.character();
                        }
                    } else if name.ends_with("DocumentCommand") {
                        scan.group(b'{'); // xparse argument specification
                    } else {
                        scan.group(b'['); // number of arguments
                        scan.group(b'['); // default first argument
                    }
                    scan.group(b'{'); // definitions are not executed while indexing
                    continue;
                }
                if matches!(
                    name,
                    "newenvironment"
                        | "renewenvironment"
                        | "provideenvironment"
                        | "NewDocumentEnvironment"
                        | "RenewDocumentEnvironment"
                        | "ProvideDocumentEnvironment"
                        | "DeclareDocumentEnvironment"
                ) {
                    if let Some(environment) = scan.group(b'{') {
                        index.symbols.push(SymbolEntry {
                            path: path.into(),
                            line: lines.line_at(start),
                            name: environment,
                            kind: SymbolKind::Definition,
                        });
                    }
                    if name.ends_with("DocumentEnvironment") {
                        scan.group(b'{');
                    } else {
                        scan.group(b'[');
                        scan.group(b'[');
                    }
                    scan.group(b'{');
                    scan.group(b'{');
                    continue;
                }
                let level = match name {
                    "part" => Some(0),
                    "chapter" => Some(1),
                    "section" => Some(2),
                    "subsection" => Some(3),
                    "subsubsection" => Some(4),
                    "paragraph" => Some(5),
                    "subparagraph" => Some(6),
                    _ => None,
                };
                if let Some(level) = level {
                    scan.group(b'['); // short running title
                    let content = (scan.peek_trivia() == Some(b'{')).then_some(scan.at + 1);
                    if let Some(title) = scan.group(b'{') {
                        if !title.trim().is_empty() {
                            index.outline.push(OutlineEntry {
                                path: path.into(),
                                line: lines.line_at(start),
                                level,
                                title: title.trim().into(),
                            });
                        }
                        // Unlike definition/literal bodies, heading contents
                        // execute. Visit labels and references at their original
                        // byte positions, without replaying the section command.
                        if let Some(content) = content {
                            scan.at = content;
                        }
                    }
                } else if matches!(name, "usepackage" | "RequirePackage") {
                    scan.group(b'[');
                    if let Some(names) = scan.group(b'{') {
                        packages.extend(
                            names
                                .split(',')
                                .map(str::trim)
                                .filter(|name| literal_name(name))
                                .map(str::to_owned),
                        );
                    }
                    scan.group(b'['); // optional package release date
                } else if matches!(
                    name,
                    "label"
                        | "ref"
                        | "eqref"
                        | "autoref"
                        | "pageref"
                        | "nameref"
                        | "cref"
                        | "Cref"
                        | "cpageref"
                        | "Cpageref"
                ) {
                    if let Some(labels) = scan.group(b'{') {
                        let entries = if name == "label" {
                            &mut index.tags
                        } else {
                            &mut index.references
                        };
                        for label in labels
                            .split(',')
                            .map(str::trim)
                            .filter(|label| literal_name(label))
                        {
                            entries.push(ReferenceEntry {
                                path: path.into(),
                                line: lines.line_at(start),
                                label: label.into(),
                            });
                        }
                    }
                } else if matches!(name, "input" | "include" | "subfile" | "subfileinclude") {
                    if matches!(name, "subfile" | "subfileinclude") {
                        scan.group(b'[');
                    }
                    let target = if scan.peek_trivia() == Some(b'{') {
                        scan.group(b'{')
                    } else if name == "input" {
                        scan.bare_path()
                    } else {
                        None
                    };
                    if target.as_deref().map(str::trim).is_none_or(|target| {
                        !literal_path(target)
                            || !local_file(target, matches!(name, "subfile" | "subfileinclude"))
                    }) {
                        index.unresolved_dependencies.push(UnresolvedDependency {
                            path: path.into(),
                            line: lines.line_at(start),
                            kind: DependencyKind::Include,
                            expression: source[start..scan.at.max(start + 1)].trim().into(),
                        });
                    }
                }
            }
            _ => scan.character(),
        }
    }
    !scan.aborted
}

fn literal_name(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || "\\{}[]#$%~^".contains(c))
}
fn literal_path(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(|c| "\\{}[]#$%~^\n\r\0".contains(c))
}

pub(super) fn resolve(
    root: &Path,
    context: &Path,
    target: &str,
    paths: &mut PathNormalizer,
) -> Option<PathBuf> {
    let mut target = PathBuf::from(target.trim().trim_matches('"'));
    if target.is_absolute() {
        return None;
    }
    if target.extension().is_none() {
        target.set_extension("tex");
    }
    // Reject lexical escapes even when a missing path cannot be canonicalized.
    let mut relative = context.strip_prefix(root).ok()?.to_owned();
    for component in target.components() {
        match component {
            Component::Normal(name) => relative.push(name),
            Component::CurDir => {}
            Component::ParentDir if relative.pop() => {}
            _ => return None,
        }
    }
    // Ordinary inputs inherit the current TeX import context.
    let candidate = paths.normalize(&root.join(relative));
    candidate.starts_with(root).then_some(candidate)
}

struct Scanner<'a, C> {
    source: &'a str,
    at: usize,
    checked: usize,
    aborted: bool,
    cancelled: &'a mut C,
}
impl<'a, C: FnMut() -> bool> Scanner<'a, C> {
    fn check(&mut self) -> bool {
        if self.at >= self.checked {
            self.checked = self.at.saturating_add(4096);
            self.aborted |= (self.cancelled)();
        }
        !self.aborted
    }
    fn character(&mut self) {
        self.at += self.source[self.at..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
    }
    fn comment(&mut self) {
        self.at = self.source[self.at..]
            .find('\n')
            .map_or(self.source.len(), |n| self.at + n + 1);
    }
    fn command(&mut self) -> &'a str {
        let start = self.at + 1;
        self.at = start;
        while self
            .source
            .as_bytes()
            .get(self.at)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'@')
        {
            self.at += 1;
        }
        if self.at == start {
            self.character();
        }
        &self.source[start..self.at]
    }
    fn star(&mut self) {
        if self.source.as_bytes().get(self.at) == Some(&b'*') {
            self.at += 1;
        }
    }
    fn peek_trivia(&mut self) -> Option<u8> {
        while self.at < self.source.len() && self.check() {
            match self.source.as_bytes()[self.at] {
                b'%' => self.comment(),
                byte if byte.is_ascii_whitespace() => self.at += 1,
                _ => break,
            }
        }
        self.source.as_bytes().get(self.at).copied()
    }
    fn group(&mut self, open: u8) -> Option<String> {
        if self.peek_trivia() != Some(open) {
            return None;
        }
        self.at += 1;
        let close = if open == b'{' { b'}' } else { b']' };
        let mut depth = 1usize;
        let mut braces = 0usize;
        let mut value = String::new();
        while self.at < self.source.len() && self.check() {
            let start = self.at;
            let byte = self.source.as_bytes()[self.at];
            if byte == b'%' {
                self.comment();
                continue;
            }
            if byte == b'\\' {
                self.command();
                value.push_str(&self.source[start..self.at]);
                continue;
            }
            if open == b'[' {
                if byte == b'{' {
                    braces += 1;
                }
                if byte == b'}' {
                    braces = braces.saturating_sub(1);
                }
            }
            if braces == 0 {
                if byte == open {
                    depth += 1;
                }
                if byte == close {
                    depth -= 1;
                    if depth == 0 {
                        self.at += 1;
                        return Some(value);
                    }
                }
            }
            self.character();
            value.push_str(&self.source[start..self.at]);
        }
        None
    }
    // Literal payloads do not interpret percent signs or nested commands.
    // Keep this cancellation-aware rather than allocating a discarded group.
    fn literal_group(&mut self, open: u8) -> bool {
        let close = if open == b'{' { b'}' } else { b']' };
        self.at += 1;
        let mut depth = 1usize;
        let mut braces = 0usize;
        while self.at < self.source.len() && self.check() {
            let byte = self.source.as_bytes()[self.at];
            if open == b'[' && matches!(byte, b'\n' | b'\r') {
                return false;
            }
            if byte == b'\\' {
                self.at += 1;
                self.character();
                continue;
            }
            if open == b'[' && byte == b'{' {
                braces += 1;
            } else if open == b'[' && byte == b'}' {
                braces = braces.saturating_sub(1);
            } else if braces == 0 && byte == open {
                depth += 1;
            } else if braces == 0 && byte == close {
                depth -= 1;
                if depth == 0 {
                    self.at += 1;
                    return true;
                }
            }
            self.character();
        }
        false
    }
    fn inline_literal(&mut self, name: &str) {
        self.star();
        if name != "verb" {
            self.peek_trivia();
            if self.source.as_bytes().get(self.at) == Some(&b'[') {
                if !self.literal_group(b'[') {
                    return;
                }
                self.peek_trivia();
            }
            if name == "mintinline" {
                if self.source.as_bytes().get(self.at) != Some(&b'{') || !self.literal_group(b'{') {
                    return;
                }
                // A percent sign can now be the payload's literal delimiter.
                while self.source[self.at..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
                    && self.check()
                {
                    self.character();
                }
            }
            if self.source.as_bytes().get(self.at) == Some(&b'{') {
                self.literal_group(b'{');
                return;
            }
        }
        if let Some(delimiter) = self.source[self.at..].chars().next()
            && !matches!(delimiter, '\n' | '\r')
        {
            self.at += delimiter.len_utf8();
            while self.at < self.source.len() && self.check() {
                if self.source[self.at..].starts_with(delimiter) {
                    self.at += delimiter.len_utf8();
                    return;
                }
                if matches!(self.source.as_bytes()[self.at], b'\n' | b'\r') {
                    return;
                }
                self.character();
            }
        }
    }
    fn comment_environment(&mut self, closing: &str) {
        let mut line_start = self.source[..self.at]
            .rfind(['\n', '\r'])
            .map_or(0, |at| at + 1);
        while self.at < self.source.len() && self.check() {
            if matches!(self.source.as_bytes()[self.at], b'\n' | b'\r') {
                if self.source[line_start..self.at].trim() == closing {
                    return;
                }
                self.at += 1;
                line_start = self.at;
            } else {
                self.character();
            }
        }
    }
    fn macro_name(&mut self) -> Option<String> {
        if self.peek_trivia() == Some(b'{') {
            return self.group(b'{').filter(|name| name.starts_with('\\'));
        }
        if self.peek_trivia() == Some(b'\\') {
            return Some(format!("\\{}", self.command()));
        }
        None
    }
    fn bare_path(&mut self) -> Option<String> {
        self.peek_trivia();
        if self.source.as_bytes().get(self.at) == Some(&b'"') {
            self.at += 1;
            let start = self.at;
            while self.at < self.source.len() && self.check() {
                if self.source.as_bytes()[self.at] == b'"' {
                    let value = self.source[start..self.at].into();
                    self.at += 1;
                    return Some(value);
                }
                self.character();
            }
            return None;
        }
        let start = self.at;
        while self.at < self.source.len() && self.check() {
            let byte = self.source.as_bytes()[self.at];
            if byte.is_ascii_whitespace() || matches!(byte, b'%' | b'\\' | b'{' | b'}') {
                break;
            }
            self.character();
        }
        (self.at > start).then(|| self.source[start..self.at].into())
    }
}

#[cfg(test)]
mod tests;
