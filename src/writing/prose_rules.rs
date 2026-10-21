//! Declarative, balanced TeX prose admission shared by automatic and selected-text checks.
//! These rules describe arguments; they never execute or expand document macros.
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, ops::Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TexArgumentRole {
    InlineProse,
    SeparateProse,
    Math,
    Identifier,
    Path,
    Literal,
}
impl TexArgumentRole {
    pub(crate) const ALL: [Self; 6] = [
        Self::InlineProse,
        Self::SeparateProse,
        Self::Math,
        Self::Identifier,
        Self::Path,
        Self::Literal,
    ];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::InlineProse => "Inline prose",
            Self::SeparateProse => "Separate passage",
            Self::Math => "Mathematics",
            Self::Identifier => "Identifier",
            Self::Path => "Path or URL",
            Self::Literal => "Literal code",
        }
    }
    fn prose(self) -> bool {
        matches!(self, Self::InlineProse | Self::SeparateProse)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TexCommandRule {
    pub(crate) name: String,
    pub(crate) arguments: Vec<TexArgumentRole>,
    #[serde(default)]
    pub(crate) optional_arguments: Vec<TexArgumentRole>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TexEnvironmentRule {
    pub(crate) name: String,
    pub(crate) body: TexArgumentRole,
    #[serde(default)]
    pub(crate) arguments: Vec<TexArgumentRole>,
    #[serde(default)]
    pub(crate) optional_arguments: Vec<TexArgumentRole>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TexProseRules {
    pub(crate) commands: Vec<TexCommandRule>,
    pub(crate) environments: Vec<TexEnvironmentRule>,
}
impl Default for TexProseRules {
    fn default() -> Self {
        use TexArgumentRole::{
            Identifier as Id, InlineProse as Inline, Literal, Path, SeparateProse as Separate,
        };
        let mut commands = Vec::new();
        for name in [
            "emph",
            "textbf",
            "textit",
            "textnormal",
            "textrm",
            "textsf",
            "texttt",
            "textup",
            "textsl",
            "textsc",
            "textmd",
            "textlf",
            "text",
            "underline",
            "sout",
            "uline",
            "uuline",
            "uwave",
            "textsuperscript",
            "textsubscript",
            "mbox",
            "hbox",
            "fbox",
        ] {
            commands.push(TexCommandRule {
                name: name.into(),
                arguments: vec![Inline],
                optional_arguments: vec![],
            });
        }
        for name in [
            "part",
            "chapter",
            "section",
            "subsection",
            "subsubsection",
            "paragraph",
            "subparagraph",
            "title",
            "author",
            "caption",
            "captionof",
            "footnote",
            "footnotetext",
            "thanks",
            "intertext",
            "shortintertext",
        ] {
            let arguments = if name == "captionof" {
                vec![Id, Separate]
            } else {
                vec![Separate]
            };
            let optional_arguments = if matches!(name, "footnote" | "footnotetext") {
                vec![Id]
            } else {
                vec![Separate]
            };
            commands.push(TexCommandRule {
                name: name.into(),
                arguments,
                optional_arguments,
            });
        }
        for (name, arguments, optional_arguments) in [
            ("href", vec![Path, Inline], vec![Id]),
            ("hyperref", vec![Inline], vec![Id]),
            ("textcolor", vec![Id, Inline], vec![Id]),
            ("colorbox", vec![Id, Inline], vec![Id]),
            ("fcolorbox", vec![Id, Id, Inline], vec![Id]),
            ("foreignlanguage", vec![Id, Inline], vec![]),
            ("texorpdfstring", vec![Inline, Id], vec![]),
            ("newtheorem", vec![Id, Separate], vec![Id, Id]),
            ("newcommand", vec![Id, Literal], vec![Id, Literal]),
            ("renewcommand", vec![Id, Literal], vec![Id, Literal]),
            ("providecommand", vec![Id, Literal], vec![Id, Literal]),
            ("DeclareRobustCommand", vec![Id, Literal], vec![Id, Literal]),
            (
                "newenvironment",
                vec![Id, Literal, Literal],
                vec![Id, Literal],
            ),
            (
                "renewenvironment",
                vec![Id, Literal, Literal],
                vec![Id, Literal],
            ),
        ] {
            commands.push(TexCommandRule {
                name: name.into(),
                arguments,
                optional_arguments,
            });
        }
        for name in [
            "label",
            "ref",
            "eqref",
            "pageref",
            "autoref",
            "cref",
            "Cref",
            "vref",
            "url",
            "nolinkurl",
            "path",
            "input",
            "include",
            "includegraphics",
            "bibliography",
            "bibliographystyle",
            "addbibresource",
            "documentclass",
            "usepackage",
            "RequirePackage",
            "PassOptionsToPackage",
            "setlength",
            "addtolength",
            "setcounter",
            "addtocounter",
            "color",
            "definecolor",
            "hypersetup",
            "lstset",
            "geometry",
            "index",
            "gls",
            "Gls",
            "acrshort",
        ] {
            let arguments = match name {
                "PassOptionsToPackage"
                | "setlength"
                | "addtolength"
                | "setcounter"
                | "addtocounter" => vec![Id, Id],
                "definecolor" => vec![Id, Id, Id],
                _ => vec![if matches!(
                    name,
                    "url"
                        | "nolinkurl"
                        | "path"
                        | "input"
                        | "include"
                        | "includegraphics"
                        | "addbibresource"
                ) {
                    Path
                } else {
                    Id
                }],
            };
            commands.push(TexCommandRule {
                name: name.into(),
                arguments,
                optional_arguments: vec![Id],
            });
        }
        for name in [
            "cite",
            "citep",
            "citet",
            "parencite",
            "textcite",
            "autocite",
            "footcite",
            "nocite",
        ] {
            commands.push(TexCommandRule {
                name: name.into(),
                arguments: vec![Id],
                optional_arguments: vec![Separate, Separate],
            });
        }
        // Declarations have no arguments. A following group belongs to prose.
        for name in [
            "centering",
            "raggedright",
            "raggedleft",
            "bfseries",
            "itshape",
            "normalfont",
            "small",
            "large",
            "Large",
            "TeX",
            "LaTeX",
            "maketitle",
            "par",
            "item",
            "noindent",
            "ignorespaces",
        ] {
            commands.push(TexCommandRule {
                name: name.into(),
                arguments: vec![],
                optional_arguments: if name == "item" { vec![Id] } else { vec![] },
            });
        }
        let mut environments = Vec::new();
        for name in [
            "document",
            "abstract",
            "quote",
            "quotation",
            "center",
            "flushleft",
            "flushright",
            "itemize",
            "enumerate",
            "description",
            "proof",
            "theorem",
            "lemma",
            "proposition",
            "corollary",
            "definition",
            "remark",
            "example",
            "figure",
            "table",
        ] {
            environments.push(TexEnvironmentRule {
                name: name.into(),
                body: Separate,
                arguments: vec![],
                optional_arguments: vec![if matches!(name, "figure" | "table" | "enumerate") {
                    Id
                } else {
                    Separate
                }],
            });
        }
        for (name, arguments) in [
            ("tabular", vec![Id]),
            ("tabular*", vec![Id, Id]),
            ("tabularx", vec![Id, Id]),
            ("tabularray", vec![Id]),
            ("tblr", vec![Id]),
            ("longtable", vec![Id]),
        ] {
            environments.push(TexEnvironmentRule {
                name: name.into(),
                body: Separate,
                arguments,
                optional_arguments: vec![Id],
            });
        }
        for name in [
            "verbatim",
            "verbatim*",
            "lstlisting",
            "minted",
            "Verbatim",
            "comment",
        ] {
            environments.push(TexEnvironmentRule {
                name: name.into(),
                body: Literal,
                arguments: vec![],
                optional_arguments: vec![],
            });
        }
        Self {
            commands,
            environments,
        }
    }
}
impl TexProseRules {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.commands.len() > 256 || self.environments.len() > 128 {
            return Err("TeX prose rules allow at most 256 commands and 128 environments.".into());
        }
        fn validate_name(name: &str, environment: bool) -> bool {
            !name.is_empty()
                && name.len() <= 80
                && name.bytes().all(|ch| {
                    ch.is_ascii_alphabetic()
                        || ch == b'@'
                        || (environment && matches!(ch, b'*' | b'-' | b'_' | b'0'..=b'9'))
                })
        }
        let mut names = std::collections::HashSet::new();
        for (index, rule) in self.commands.iter().enumerate() {
            if !validate_name(&rule.name, false) {
                return Err(format!(
                    "TeX command rule {} needs a command name without a backslash (letters or @, at most 80 characters).",
                    index + 1
                ));
            }
            if !names.insert(rule.name.as_str()) {
                return Err(format!("Duplicate TeX command rule: \\{}.", rule.name));
            }
            if rule.arguments.len() + rule.optional_arguments.len() > 16 {
                return Err(format!(
                    "TeX command \\{} has more than 16 argument rules.",
                    rule.name
                ));
            }
        }
        names.clear();
        for (index, rule) in self.environments.iter().enumerate() {
            if !validate_name(&rule.name, true) {
                return Err(format!(
                    "TeX environment rule {} has an invalid name (at most 80 characters).",
                    index + 1
                ));
            }
            if !names.insert(rule.name.as_str()) {
                return Err(format!("Duplicate TeX environment rule: {}.", rule.name));
            }
            if rule.arguments.len() + rule.optional_arguments.len() > 16 {
                return Err(format!(
                    "TeX environment {} has more than 16 argument rules.",
                    rule.name
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ProseRegion {
    Prose(Range<usize>),
    Opaque(Range<usize>),
    Boundary(usize),
}

/// A single balanced-delimiter prepass gives constant-time argument boundaries;
/// deeply nested formatting therefore cannot repeatedly scan the same suffix.
/// Parsing depth is capped to prevent an unfinished document overflowing the stack.
pub(super) fn regions(source: &[char], rules: &TexProseRules) -> Vec<ProseRegion> {
    let mut scanner = Scanner {
        source,
        groups: group_index(source),
        commands: rules
            .commands
            .iter()
            .map(|rule| (rule.name.chars().collect::<Vec<_>>(), rule))
            .collect(),
        environments: rules
            .environments
            .iter()
            .map(|rule| (rule.name.chars().collect::<Vec<_>>(), rule))
            .collect(),
        result: Vec::new(),
    };
    scanner.scan(0, source.len(), 0, None);
    scanner.result
}
struct Scanner<'a> {
    source: &'a [char],
    groups: Vec<usize>,
    commands: HashMap<Vec<char>, &'a TexCommandRule>,
    environments: HashMap<Vec<char>, &'a TexEnvironmentRule>,
    result: Vec<ProseRegion>,
}
impl Scanner<'_> {
    fn scan(
        &mut self,
        mut cursor: usize,
        end: usize,
        depth: usize,
        stop_env: Option<&[char]>,
    ) -> usize {
        if depth >= 128 {
            self.result.push(ProseRegion::Opaque(cursor..end));
            return end;
        }
        let mut plain = cursor;
        while cursor < end {
            if !matches!(self.source[cursor], '\\' | '%' | '{' | '}') {
                cursor += 1;
                continue;
            }
            self.prose(plain..cursor);
            if self.source[cursor] == '%' {
                // Comments may contain prose with TeX markup, but cannot carry
                // syntax across the line boundary. Magic directives are metadata.
                let start = cursor + 1;
                cursor = start;
                while cursor < end && self.source[cursor] != '\n' {
                    cursor += 1;
                }
                let body = &self.source[start..cursor];
                if body.iter().find(|ch| !ch.is_whitespace()) != Some(&'!') {
                    self.scan(start, cursor, depth + 1, None);
                }
            } else if self.source[cursor] == '{' {
                // A naked group changes TeX scope, not the meaning of its prose.
                let (body, after) = self.group(cursor, end);
                self.scan(body.start, body.end, depth + 1, None);
                cursor = after;
            } else if self.source[cursor] == '}' {
                cursor += 1;
            } else {
                let command_start = cursor;
                let (name, next) = command(self.source, cursor, end);
                cursor = next;
                if name.iter().copied().eq("end".chars()) {
                    if let Some((body, after)) = self.next_group(cursor, end, '{') {
                        let env = &self.source[body];
                        cursor = after;
                        if stop_env == Some(env) {
                            return cursor;
                        }
                    }
                } else if name.iter().copied().eq("begin".chars()) {
                    if let Some((body, after)) = self.next_group(cursor, end, '{') {
                        let env = &self.source[body];
                        let rule = self.environments.get(env).copied();
                        let role = rule.map_or(TexArgumentRole::SeparateProse, |rule| rule.body);
                        if role == TexArgumentRole::SeparateProse {
                            self.boundary(command_start);
                        }
                        cursor = if let Some(rule) = rule {
                            self.arguments(
                                after,
                                end,
                                &rule.arguments,
                                &rule.optional_arguments,
                                depth,
                            )
                        } else {
                            self.unknown_arguments(after, end)
                        };
                        if role.prose() {
                            cursor = self.scan(cursor, end, depth + 1, Some(env));
                        } else {
                            let after = environment_end(self.source, cursor, end, env);
                            self.result.push(ProseRegion::Opaque(cursor..after));
                            cursor = after;
                        }
                        if role == TexArgumentRole::SeparateProse {
                            self.boundary(cursor);
                        }
                    }
                } else if matches!(
                    name,
                    ['d', 'e', 'f']
                        | ['g', 'd', 'e', 'f']
                        | ['e', 'd', 'e', 'f']
                        | ['x', 'd', 'e', 'f']
                ) {
                    // Primitive definitions have an unbraced parameter preamble.
                    while cursor < end && self.source[cursor] != '{' {
                        cursor += 1;
                    }
                    if cursor < end {
                        cursor = self.group(cursor, end).1;
                    }
                } else if name.iter().all(|ch| ch.is_ascii_alphabetic() || *ch == '@') {
                    if matches!(
                        name,
                        ['p', 'a', 'r']
                            | ['i', 't', 'e', 'm']
                            | ['m', 'a', 'k', 'e', 't', 'i', 't', 'l', 'e']
                    ) {
                        self.boundary(command_start);
                    }
                    if self.source.get(cursor) == Some(&'*') {
                        cursor += 1;
                    }
                    cursor = if let Some(rule) = self.commands.get(name).copied() {
                        self.arguments(
                            cursor,
                            end,
                            &rule.arguments,
                            &rule.optional_arguments,
                            depth,
                        )
                    } else {
                        self.unknown_arguments(cursor, end)
                    };
                }
            }
            plain = cursor;
        }
        self.prose(plain..end);
        end
    }
    fn group(&self, start: usize, end: usize) -> (Range<usize>, usize) {
        let close = self.groups[start];
        if close < end {
            (start + 1..close, close + 1)
        } else {
            (start + 1..end, end)
        }
    }
    fn next_group(&self, cursor: usize, end: usize, open: char) -> Option<(Range<usize>, usize)> {
        let next = trivia(self.source, cursor, end);
        (next < end && self.source[next] == open).then(|| self.group(next, end))
    }
    fn prose(&mut self, range: Range<usize>) {
        if !range.is_empty() {
            self.result.push(ProseRegion::Prose(range));
        }
    }
    fn boundary(&mut self, at: usize) {
        self.result.push(ProseRegion::Boundary(at));
    }
    fn arguments(
        &mut self,
        mut cursor: usize,
        end: usize,
        required: &[TexArgumentRole],
        optional: &[TexArgumentRole],
        depth: usize,
    ) -> usize {
        let (mut arg, mut opt) = (0, 0);
        loop {
            let next = trivia(self.source, cursor, end);
            let opening = self.source.get(next).copied();
            let role = match opening {
                Some('[') if opt < optional.len() => {
                    let role = optional[opt];
                    opt += 1;
                    role
                }
                Some('{') if arg < required.len() => {
                    let role = required[arg];
                    arg += 1;
                    role
                }
                _ => break,
            };
            let (body, after) = self.group(next, end);
            if role == TexArgumentRole::SeparateProse {
                self.boundary(next);
            }
            if role.prose() {
                self.scan(body.start, body.end, depth + 1, None);
            } else if matches!(role, TexArgumentRole::Math | TexArgumentRole::Literal) {
                self.result.push(ProseRegion::Opaque(body));
            }
            if role == TexArgumentRole::SeparateProse {
                self.boundary(after);
            }
            cursor = after;
        }
        cursor
    }
    fn unknown_arguments(&mut self, mut cursor: usize, end: usize) -> usize {
        loop {
            let next = trivia(self.source, cursor, end);
            let Some(&('{' | '[')) = self.source.get(next) else {
                break;
            };
            let (body, after) = self.group(next, end);
            self.result.push(ProseRegion::Opaque(body));
            cursor = after;
        }
        cursor
    }
}
fn group_index(source: &[char]) -> Vec<usize> {
    let mut result = vec![source.len(); source.len()];
    index_groups(source, 0..source.len(), &mut result, true);
    result
}
fn index_groups(source: &[char], range: Range<usize>, result: &mut [usize], comments: bool) {
    let (mut braces, mut brackets) = (Vec::new(), Vec::new());
    let mut cursor = range.start;
    while cursor < range.end {
        match source[cursor] {
            '\\' => cursor = (cursor + 2).min(range.end),
            '%' if comments => {
                let start = cursor + 1;
                while cursor < range.end && source[cursor] != '\n' {
                    cursor += 1;
                }
                // Comment prose has independent markup. Its braces may never
                // close a command in the actual document (or on another line).
                index_groups(source, start..cursor, result, false);
            }
            '{' => {
                braces.push(cursor);
                cursor += 1;
            }
            '}' => {
                if let Some(open) = braces.pop() {
                    result[open] = cursor;
                }
                while brackets
                    .last()
                    .is_some_and(|&(_, depth)| depth > braces.len())
                {
                    brackets.pop();
                }
                cursor += 1;
            }
            '[' => {
                brackets.push((cursor, braces.len()));
                cursor += 1;
            }
            ']' => {
                if brackets
                    .last()
                    .is_some_and(|&(_, depth)| depth == braces.len())
                {
                    let (open, _) = brackets.pop().unwrap();
                    result[open] = cursor;
                }
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }
}

fn command(source: &[char], start: usize, end: usize) -> (&[char], usize) {
    let begin = (start + 1).min(end);
    let mut cursor = begin;
    while cursor < end && (source[cursor].is_ascii_alphabetic() || source[cursor] == '@') {
        cursor += 1;
    }
    if cursor == begin {
        cursor = (cursor + 1).min(end);
    }
    (&source[begin..cursor], cursor)
}
fn trivia(source: &[char], mut cursor: usize, end: usize) -> usize {
    loop {
        while cursor < end && source[cursor].is_whitespace() {
            cursor += 1;
        }
        if source.get(cursor) != Some(&'%') || cursor >= end {
            return cursor;
        }
        while cursor < end && source[cursor] != '\n' {
            cursor += 1;
        }
    }
}
fn next_group(
    source: &[char],
    cursor: usize,
    end: usize,
    open: char,
    close: char,
) -> Option<(Range<usize>, usize)> {
    let cursor = trivia(source, cursor, end);
    (cursor < end && source[cursor] == open).then(|| group(source, cursor, end, open, close))
}
fn group(
    source: &[char],
    start: usize,
    end: usize,
    open: char,
    close: char,
) -> (Range<usize>, usize) {
    let mut cursor = start + 1;
    let mut depth = 1;
    let mut braces = 0;
    while cursor < end {
        match source[cursor] {
            '\\' => cursor = (cursor + 2).min(end),
            '%' => {
                while cursor < end && source[cursor] != '\n' {
                    cursor += 1;
                }
            }
            '{' if open == '[' => {
                braces += 1;
                cursor += 1;
            }
            '}' if open == '[' && braces > 0 => {
                braces -= 1;
                cursor += 1;
            }
            ch if ch == open && braces == 0 => {
                depth += 1;
                cursor += 1;
            }
            ch if ch == close && braces == 0 => {
                depth -= 1;
                if depth == 0 {
                    return (start + 1..cursor, cursor + 1);
                }
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }
    (start + 1..end, end)
}
pub(super) fn environment_end(
    source: &[char],
    mut cursor: usize,
    end: usize,
    env: &[char],
) -> usize {
    let mut nesting = 1;
    while cursor < end {
        if source[cursor] == '%' {
            while cursor < end && source[cursor] != '\n' {
                cursor += 1;
            }
        } else if source[cursor] == '\\' {
            let (name, after) = command(source, cursor, end);
            cursor = after;
            if (name.iter().copied().eq("begin".chars()) || name.iter().copied().eq("end".chars()))
                && let Some((body, after)) = next_group(source, cursor, end, '{', '}')
                && source[body] == *env
            {
                cursor = after;
                if name.iter().copied().eq("begin".chars()) {
                    nesting += 1;
                } else {
                    nesting -= 1;
                    if nesting == 0 {
                        return cursor;
                    }
                }
            }
        } else {
            cursor += 1;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prose(source: &str, rules: &TexProseRules) -> String {
        let chars: Vec<_> = source.chars().collect();
        regions(&chars, rules)
            .iter()
            .filter_map(|region| match region {
                ProseRegion::Prose(range) => Some(chars[range.clone()].iter().collect::<String>()),
                _ => None,
            })
            .collect()
    }
    #[test]
    fn rules_validate_names_duplicates_counts_and_enum_values() {
        let mut rules = TexProseRules::default();
        assert!(rules.validate().is_ok());
        rules.commands.push(rules.commands[0].clone());
        assert!(rules.validate().unwrap_err().contains("Duplicate"));
        rules.commands.pop();
        rules.commands[0].name = "\\emph".into();
        assert!(
            rules
                .validate()
                .unwrap_err()
                .contains("without a backslash")
        );
        assert!(serde_json::from_str::<TexArgumentRole>("\"invent\"").is_err());
    }
    #[test]
    fn custom_arguments_are_balanced_and_typed() {
        let mut rules = TexProseRules::default();
        rules.commands.push(TexCommandRule {
            name: "mycomment".into(),
            arguments: vec![TexArgumentRole::Identifier, TexArgumentRole::InlineProse],
            optional_arguments: vec![],
        });
        assert_eq!(
            prose(
                r"Before \mycomment {nested{hidden}} {visible \emph{words}} after.",
                &rules
            ),
            "Before visible words after."
        );
    }
    #[test]
    fn configured_environment_body_and_metadata_are_independent() {
        let mut rules = TexProseRules::default();
        rules.environments.push(TexEnvironmentRule {
            name: "custom".into(),
            body: TexArgumentRole::InlineProse,
            arguments: vec![TexArgumentRole::Identifier],
            optional_arguments: vec![],
        });
        assert_eq!(
            prose(
                r"Before \begin{custom}{technical}visible \emph{words}\end{custom} after.",
                &rules
            ),
            "Before visible words after."
        );
        rules.environments.last_mut().unwrap().body = TexArgumentRole::Literal;
        assert_eq!(
            prose(
                r"Before \begin{custom}{technical}hidden \begin{custom}nested\end{custom} hidden\end{custom} after.",
                &rules
            ),
            "Before  after."
        );
    }
    #[test]
    fn optional_groups_respect_nested_braces_comments_and_escapes() {
        assert_eq!(
            prose(
                "Before \\textcolor [named{[x]}] % ignored }\n {red}{visible \\{words\\}} after.",
                &TexProseRules::default()
            ),
            "Before visible words after."
        );
    }
    #[test]
    fn argument_index_caches_every_nested_close_and_ignores_comment_delimiters() {
        let source = format!("{}body{}", "\\emph{".repeat(1024), "}".repeat(1024));
        let chars: Vec<_> = source.chars().collect();
        let index = group_index(&chars);
        for (depth, open) in chars
            .iter()
            .enumerate()
            .filter_map(|(at, ch)| (*ch == '{').then_some(at))
            .enumerate()
        {
            assert_eq!(index[open], chars.len() - 1 - depth);
        }
        let chars: Vec<_> = "[option{a]b} tail] % ignored }\n{word \\} end}"
            .chars()
            .collect();
        let index = group_index(&chars);
        assert_eq!(index[0], 17);
        assert_eq!(index[7], 11);
        let last_open = chars.iter().rposition(|ch| *ch == '{').unwrap();
        assert_eq!(index[last_open], chars.len() - 1);
    }

    #[test]
    fn comments_keep_markup_roles_but_cannot_swallow_the_next_line() {
        let source = "% A \\emph{visible} note \\label{hidden}.\nNext sentence.";
        assert_eq!(
            prose(source, &TexProseRules::default()),
            " A visible note .\nNext sentence."
        );
        let source = "% Unfinished \\unknown{hidden\nNext sentence.";
        assert_eq!(
            prose(source, &TexProseRules::default()),
            " Unfinished \nNext sentence."
        );
    }

    #[test]
    fn excessive_nesting_is_bounded_without_leaking_commands() {
        let source = format!(
            "{}secret{} after.",
            "\\emph{".repeat(1024),
            "}".repeat(1024)
        );
        let actual = prose(&source, &TexProseRules::default());
        assert!(!actual.contains("secret"));
        assert!(actual.contains("after"));
    }
}
