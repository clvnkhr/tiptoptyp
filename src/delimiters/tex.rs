//! A revision-derived TeX lexical index. It deliberately performs no expansion.
use std::ops::Range;

use crate::rainbow::BracketFamily;

#[derive(Debug, Clone)]
struct Endpoint {
    bytes: Range<usize>,
    mate: Range<usize>,
    family: Option<BracketFamily>,
    depth: usize,
}

#[derive(Debug)]
struct Ignored {
    bytes: Range<usize>,
    includes_end: bool,
    environment: Option<LiteralEnvironment>,
}

#[derive(Debug)]
struct LiteralEnvironment {
    open: Range<usize>,
    close: Option<Range<usize>>,
}

/// Construct once for a source revision. Cursor queries are binary searches.
#[derive(Debug, Default)]
pub(crate) struct TexDelimiterIndex {
    endpoints: Vec<Endpoint>,
    ignored: Vec<Range<usize>>,
    ignored_ends: Vec<usize>,
    math_regions: Vec<Range<usize>>,
    enclosing: super::EnclosingPairs,
    environment_starts: Vec<usize>,
    environment_closers_needed: Vec<usize>,
}

impl TexDelimiterIndex {
    pub(crate) fn new(source: &str) -> Self {
        let ignored = ignored_regions(source);
        let mut index = Self {
            endpoints: Vec::new(),
            ignored: ignored.iter().map(|region| region.bytes.clone()).collect(),
            ignored_ends: ignored
                .iter()
                .filter(|region| region.includes_end)
                .map(|region| region.bytes.end)
                .collect(),
            math_regions: Vec::new(),
            enclosing: Default::default(),
            environment_starts: Vec::new(),
            environment_closers_needed: Vec::new(),
        };
        for environment in ignored
            .iter()
            .filter_map(|region| region.environment.as_ref())
        {
            index.environment_starts.push(environment.open.start);
            if let Some(close) = &environment.close {
                index.pair(
                    environment.open.clone(),
                    close.clone(),
                    Some(BracketFamily::Curly),
                );
            } else {
                index
                    .environment_closers_needed
                    .push(environment.open.start);
            }
        }
        let mut scanner = Scanner::default();
        struct Header {
            start: usize,
            same_name_ancestor: Option<usize>,
            matched: bool,
        }
        let mut headers: Vec<Header> = Vec::new();
        let mut name_heads = std::collections::BTreeMap::new();
        let mut environments: Vec<(&str, Range<usize>, usize)> = Vec::new();
        let mut ignored_index = 0;
        let mut at = 0;
        while at < source.len() {
            if let Some(region) = index.ignored.get(ignored_index) {
                if at >= region.end {
                    ignored_index += 1;
                    continue;
                }
                if at >= region.start {
                    at = region.end;
                    continue;
                }
            }
            let rest = &source[at..];
            let ch = rest.chars().next().unwrap();
            match ch {
                '{' => scanner.groups.push(Group {
                    bytes: at..at + 1,
                    visible: scanner.visible.len(),
                    math: scanner.math.len(),
                }),
                '}' => {
                    if let Some(group) = scanner.groups.pop() {
                        scanner.truncate_visible(group.visible);
                        for open in scanner.math.drain(group.math..) {
                            index.math_regions.push(open.bytes.end..at);
                        }
                        index.pair(group.bytes, at..at + 1, Some(BracketFamily::Curly));
                    }
                }
                '(' | '[' => scanner.open(
                    if ch == '(' {
                        Shape::Round
                    } else {
                        Shape::Square
                    },
                    at..at + 1,
                    false,
                ),
                ')' | ']' => scanner.close(
                    if ch == ')' {
                        Shape::Round
                    } else {
                        Shape::Square
                    },
                    at..at + 1,
                    &mut index,
                ),
                '$' => {
                    let length = if rest.starts_with("$$") { 2 } else { 1 };
                    let kind = if length == 2 {
                        Math::DoubleDollar
                    } else {
                        Math::Dollar
                    };
                    scanner.math_marker(kind, at..at + length, true, &mut index);
                    at += length;
                    continue;
                }
                '|' if scanner.in_math() => {
                    scanner.neutral(Shape::Bar, at..at + 1, &mut index);
                }
                ',' | ';' if scanner.in_math() && scanner.visible.len() > scanner.floor() => {
                    if let Some(open) = scanner.visible.last_mut() {
                        open.interval_separator = true;
                    }
                }
                '\\' => {
                    let end = control_end(source, at);
                    let command = &source[at + 1..end];
                    match command {
                        "(" => scanner.math_open(Math::Inline, at..end),
                        "[" => scanner.math_open(Math::Display, at..end),
                        ")" => scanner.math_marker(Math::Inline, at..end, false, &mut index),
                        "]" => scanner.math_marker(Math::Display, at..end, false, &mut index),
                        "begin" | "end" => {
                            if let Some((name, after)) = environment(source, end) {
                                if command == "begin" {
                                    index.environment_starts.push(at);
                                    let header = headers.len();
                                    headers.push(Header {
                                        start: at,
                                        same_name_ancestor: name_heads.insert(name, header),
                                        matched: false,
                                    });
                                    environments.push((name, at..after, header));
                                } else if environments
                                    .last()
                                    .is_some_and(|(old, _, _)| *old == name)
                                {
                                    let (_, open, header) = environments.pop().unwrap();
                                    headers[header].matched = true;
                                    if let Some(ancestor) = headers[header].same_name_ancestor {
                                        name_heads.insert(name, ancestor);
                                    } else {
                                        name_heads.remove(name);
                                    }
                                    index.pair(open, at..after, Some(BracketFamily::Curly));
                                } else {
                                    // A mismatched end cannot make crossing
                                    // pairs from malformed nested environments.
                                    environments.clear();
                                    name_heads.clear();
                                }
                                if crate::editor_math_guides::math_environment(name) {
                                    if command == "begin" {
                                        scanner.environments.push((name.to_owned(), after));
                                    } else if scanner
                                        .environments
                                        .last()
                                        .is_some_and(|(old, _)| old == name)
                                    {
                                        let (_, start) = scanner.environments.pop().unwrap();
                                        index.math_regions.push(start..at);
                                    }
                                }
                                // Name braces belong to the full environment
                                // token, rather than overlapping group spans.
                                at = after;
                                continue;
                            }
                        }
                        "left" | "right" if scanner.in_math() => {
                            let marker = skip_trivia(source, end);
                            if let Some((shape, range)) = printed_delimiter(source, marker) {
                                if command == "left" {
                                    scanner.open(shape, range.clone(), true);
                                } else {
                                    scanner.close_sized(shape, range.clone(), &mut index);
                                }
                                at = range.end;
                                continue;
                            }
                        }
                        "middle" if scanner.in_math() => {
                            if let Some((_, range)) =
                                printed_delimiter(source, skip_trivia(source, end))
                            {
                                at = range.end;
                                continue;
                            }
                        }
                        _ if scanner.in_math() => {
                            if let Some((shape, opening)) = named_shape(command) {
                                if let Some(opening) = opening {
                                    if opening {
                                        scanner.open(shape, at..end, false);
                                    } else {
                                        scanner.close(shape, at..end, &mut index);
                                    }
                                } else {
                                    scanner.neutral(shape, at..end, &mut index);
                                }
                            }
                        }
                        _ => {}
                    }
                    at = end;
                    continue;
                }
                _ => {}
            }
            at += ch.len_utf8();
        }
        index.math_regions.extend(
            scanner
                .math
                .into_iter()
                .map(|open| open.bytes.end..source.len()),
        );
        index.math_regions.extend(
            scanner
                .environments
                .into_iter()
                .map(|(_, start)| start..source.len()),
        );
        index
            .math_regions
            .sort_unstable_by_key(|region| region.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for region in std::mem::take(&mut index.math_regions) {
            if let Some(previous) = merged.last_mut()
                && region.start <= previous.end
            {
                previous.end = previous.end.max(region.end);
            } else {
                merged.push(region);
            }
        }
        index.math_regions = merged;
        index
            .endpoints
            .sort_unstable_by_key(|endpoint| endpoint.bytes.start);
        index.environment_starts.sort_unstable();
        let mut unmatched_ancestors = Vec::with_capacity(headers.len());
        for header in &headers {
            let ancestor_unmatched = header.same_name_ancestor.is_some_and(|ancestor| {
                !headers[ancestor].matched || unmatched_ancestors[ancestor]
            });
            unmatched_ancestors.push(ancestor_unmatched);
            if !header.matched || ancestor_unmatched {
                index.environment_closers_needed.push(header.start);
            }
        }
        index.environment_closers_needed.sort_unstable();
        index.enclosing = super::EnclosingPairs::new(
            index
                .endpoints
                .iter()
                .filter(|endpoint| endpoint.bytes.start < endpoint.mate.start)
                .map(|endpoint| [endpoint.bytes.clone(), endpoint.mate.clone()])
                .collect(),
        );
        let mut depths = [0_usize; 4];
        for endpoint in &mut index.endpoints {
            let Some(family) = endpoint.family else {
                continue;
            };
            let depth = &mut depths[family as usize];
            if endpoint.bytes.start > endpoint.mate.start {
                *depth = depth.saturating_sub(1);
            }
            endpoint.depth = *depth;
            if endpoint.bytes.start < endpoint.mate.start {
                *depth += 1;
            }
        }
        index
    }

    fn pair(&mut self, left: Range<usize>, right: Range<usize>, family: Option<BracketFamily>) {
        self.endpoints.push(Endpoint {
            bytes: left.clone(),
            mate: right.clone(),
            family,
            depth: 0,
        });
        self.endpoints.push(Endpoint {
            bytes: right,
            mate: left,
            family,
            depth: 0,
        });
    }

    pub(crate) fn pair_at(&self, byte: usize) -> Option<[Range<usize>; 2]> {
        let at = self
            .endpoints
            .partition_point(|endpoint| endpoint.bytes.start <= byte);
        let endpoint = self.endpoints.get(at.checked_sub(1)?)?;
        if !endpoint.bytes.contains(&byte) {
            return None;
        }
        Some(if endpoint.bytes.start < endpoint.mate.start {
            [endpoint.bytes.clone(), endpoint.mate.clone()]
        } else {
            [endpoint.mate.clone(), endpoint.bytes.clone()]
        })
    }

    pub(crate) fn pair_at_caret(&self, byte: usize) -> Option<[Range<usize>; 2]> {
        self.pair_at(byte)
            .or_else(|| {
                byte.checked_sub(1)
                    .and_then(|previous| self.pair_at(previous))
            })
            .or_else(|| self.enclosing.containing(byte))
    }

    /// A real lexical begin header, paired or incomplete at EOF. Raw outer
    /// markers are eligible; comment/URL/raw-payload lookalikes never are.
    pub(crate) fn environment_start_at(&self, byte: usize) -> bool {
        self.environment_starts.binary_search(&byte).is_ok()
    }

    /// Enter completion reserves an existing same-name end for an unfinished
    /// outer environment. Ordinary matching still pairs endpoints in LIFO order.
    pub(crate) fn environment_needs_closer(&self, byte: usize) -> bool {
        self.environment_start_at(byte)
            && self.environment_closers_needed.binary_search(&byte).is_ok()
    }

    pub(crate) fn ignored_at(&self, byte: usize) -> bool {
        let at = self.ignored.partition_point(|region| region.start <= byte);
        at.checked_sub(1)
            .and_then(|at| self.ignored.get(at))
            .is_some_and(|region| region.contains(&byte))
    }

    pub(crate) fn ignored_at_caret(&self, byte: usize) -> bool {
        self.ignored_at(byte) || self.ignored_ends.binary_search(&byte).is_ok()
    }

    pub(crate) fn ignored_ranges(&self) -> &[Range<usize>] {
        &self.ignored
    }

    pub(crate) fn math_at_caret(&self, byte: usize) -> bool {
        if self.ignored_at_caret(byte) {
            return false;
        }
        let at = self
            .math_regions
            .partition_point(|region| region.start <= byte);
        at.checked_sub(1)
            .and_then(|at| self.math_regions.get(at))
            .is_some_and(|region| byte <= region.end)
    }

    pub(crate) fn brackets(
        &self,
    ) -> impl Iterator<Item = (Range<usize>, BracketFamily, usize)> + '_ {
        self.endpoints.iter().filter_map(|endpoint| {
            endpoint
                .family
                .map(|family| (endpoint.bytes.clone(), family, endpoint.depth))
        })
    }
}

/// Reuse lexical suppression without constructing a delimiter-pair index.
pub(crate) fn tex_ignored_ranges(source: &str) -> Vec<Range<usize>> {
    ignored_regions(source)
        .into_iter()
        .map(|region| region.bytes)
        .collect()
}

fn ignored_regions(source: &str) -> Vec<Ignored> {
    let mut result = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let rest = &source[at..];
        if rest.starts_with('%') {
            let end = at + rest.find(['\n', '\r']).unwrap_or(rest.len());
            result.push(Ignored {
                bytes: at..end,
                includes_end: true,
                environment: None,
            });
            at = end;
        } else if rest.starts_with('\\') {
            if let Some(literal) = crate::editor_math_guides::tex_inline_literal(source, at) {
                result.push(Ignored {
                    bytes: at..literal.end,
                    includes_end: !literal.closed,
                    environment: None,
                });
                at = literal.end;
                continue;
            }
            let end = control_end(source, at);
            if matches!(&source[at + 1..end], "url" | "nolinkurl" | "path" | "href")
                && let Some((open, stop, closed)) =
                    literal_argument(source, end, &source[at + 1..end] == "href")
            {
                let mut header = end;
                while header < open {
                    if source[header..].starts_with('\\') {
                        header = control_end(source, header);
                    } else if source[header..].starts_with('%') {
                        let stop = header
                            + source[header..]
                                .find(['\n', '\r'])
                                .unwrap_or(source.len() - header);
                        result.push(Ignored {
                            bytes: header..stop,
                            includes_end: true,
                            environment: None,
                        });
                        header = stop;
                    } else {
                        header += source[header..].chars().next().unwrap().len_utf8();
                    }
                }
                // URL catcodes make percent and bracket-like payload literal.
                // Preserve the outer group for matching, but not its payload.
                result.push(Ignored {
                    bytes: open + 1..stop,
                    includes_end: true,
                    environment: None,
                });
                at = if closed { stop } else { source.len() };
                continue;
            }
            if &source[at + 1..end] == "begin"
                && let Some((name, after)) = environment(source, end)
                && literal_environment(name)
            {
                let close = format!("\\end{{{name}}}");
                let found = source[after..].find(&close);
                let stop = found.map_or(source.len(), |offset| after + offset + close.len());
                result.push(Ignored {
                    bytes: at..stop,
                    includes_end: found.is_none(),
                    environment: Some(LiteralEnvironment {
                        open: at..after,
                        close: found.map(|offset| after + offset..stop),
                    }),
                });
                at = stop;
            } else {
                at = end;
            }
        } else {
            at += rest.chars().next().unwrap().len_utf8();
        }
    }
    result
}

fn literal_environment(name: &str) -> bool {
    matches!(
        name.trim_end_matches('*'),
        "verbatim" | "Verbatim" | "BVerbatim" | "LVerbatim" | "lstlisting" | "minted" | "comment"
    )
}

fn control_end(source: &str, start: usize) -> usize {
    let mut at = start + 1;
    while source
        .as_bytes()
        .get(at)
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'@')
    {
        at += 1;
    }
    if at == start + 1 {
        at += source[at..].chars().next().map_or(0, char::len_utf8);
    }
    at
}

fn skip_trivia(source: &str, mut at: usize) -> usize {
    loop {
        while let Some(ch) = source[at..].chars().next().filter(|ch| ch.is_whitespace()) {
            at += ch.len_utf8();
        }
        if source[at..].starts_with('%') {
            at += source[at..].find(['\n', '\r']).unwrap_or(source.len() - at);
        } else {
            return at;
        }
    }
}

fn environment(source: &str, after_command: usize) -> Option<(&str, usize)> {
    let at = skip_trivia(source, after_command);
    let rest = source.get(at..)?.strip_prefix('{')?;
    let close = rest.find('}')?;
    let name = rest[..close].trim();
    (!name.is_empty()
        && !name
            .chars()
            .any(|ch| ch.is_whitespace() || "\\{}%".contains(ch)))
    .then_some((name, at + close + 2))
}

fn literal_argument(
    source: &str,
    after_command: usize,
    allow_options: bool,
) -> Option<(usize, usize, bool)> {
    let mut open = skip_trivia(source, after_command);
    if allow_options && source.as_bytes().get(open) == Some(&b'[') {
        let mut at = open + 1;
        let mut brackets = 1_usize;
        let mut braces = 0_usize;
        while at < source.len() {
            let ch = source[at..].chars().next().unwrap();
            match ch {
                '\\' => {
                    at = control_end(source, at);
                    continue;
                }
                '%' => {
                    at += source[at..].find(['\n', '\r']).unwrap_or(source.len() - at);
                    continue;
                }
                '{' => braces += 1,
                '}' => braces = braces.saturating_sub(1),
                '[' if braces == 0 => brackets += 1,
                ']' if braces == 0 => {
                    brackets -= 1;
                    if brackets == 0 {
                        open = skip_trivia(source, at + 1);
                        break;
                    }
                }
                _ => {}
            }
            at += ch.len_utf8();
        }
        if brackets != 0 {
            return None;
        }
    }
    if source.as_bytes().get(open) != Some(&b'{') {
        return None;
    }
    let mut at = open + 1;
    let mut depth = 1;
    while at < source.len() {
        let ch = source[at..].chars().next().unwrap();
        match ch {
            '\\' => {
                at = control_end(source, at);
                continue;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open, at, true));
                }
            }
            _ => {}
        }
        at += ch.len_utf8();
    }
    Some((open, at, false))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Round,
    Square,
    Curly,
    Angle,
    Ceiling,
    Floor,
    Bar,
    DoubleBar,
    Invisible,
}
impl Shape {
    fn family(self) -> BracketFamily {
        match self {
            Self::Round => BracketFamily::Round,
            Self::Square => BracketFamily::Square,
            Self::Curly => BracketFamily::Curly,
            _ => BracketFamily::Mixed,
        }
    }
}

fn named_shape(name: &str) -> Option<(Shape, Option<bool>)> {
    Some(match name {
        "{" | "lbrace" => (Shape::Curly, Some(true)),
        "}" | "rbrace" => (Shape::Curly, Some(false)),
        "langle" => (Shape::Angle, Some(true)),
        "rangle" => (Shape::Angle, Some(false)),
        "lceil" => (Shape::Ceiling, Some(true)),
        "rceil" => (Shape::Ceiling, Some(false)),
        "lfloor" => (Shape::Floor, Some(true)),
        "rfloor" => (Shape::Floor, Some(false)),
        "lvert" => (Shape::Bar, Some(true)),
        "rvert" => (Shape::Bar, Some(false)),
        "lVert" => (Shape::DoubleBar, Some(true)),
        "rVert" => (Shape::DoubleBar, Some(false)),
        "vert" => (Shape::Bar, None),
        "|" | "Vert" => (Shape::DoubleBar, None),
        _ => return None,
    })
}

fn printed_delimiter(source: &str, start: usize) -> Option<(Shape, Range<usize>)> {
    let rest = source.get(start..)?;
    let ch = rest.chars().next()?;
    let shape = match ch {
        '(' | ')' => Shape::Round,
        '[' | ']' => Shape::Square,
        '|' => Shape::Bar,
        '.' => Shape::Invisible,
        '\\' => {
            let end = control_end(source, start);
            return named_shape(&source[start + 1..end]).map(|(shape, _)| (shape, start..end));
        }
        _ => return None,
    };
    Some((shape, start..start + ch.len_utf8()))
}

#[derive(Debug)]
struct Visible {
    shape: Shape,
    bytes: Range<usize>,
    sized: bool,
    interval_separator: bool,
}
#[derive(Debug)]
struct Group {
    bytes: Range<usize>,
    visible: usize,
    math: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Math {
    Inline,
    Display,
    Dollar,
    DoubleDollar,
}
#[derive(Debug)]
struct MathOpen {
    kind: Math,
    bytes: Range<usize>,
    visible: usize,
    groups: usize,
}
#[derive(Default)]
struct Scanner {
    visible: Vec<Visible>,
    sized: Vec<usize>,
    groups: Vec<Group>,
    math: Vec<MathOpen>,
    environments: Vec<(String, usize)>,
}
impl Scanner {
    fn in_math(&self) -> bool {
        !self.math.is_empty() || !self.environments.is_empty()
    }
    fn floor(&self) -> usize {
        self.groups.last().map_or(0, |group| group.visible)
    }
    fn open(&mut self, shape: Shape, bytes: Range<usize>, sized: bool) {
        if sized {
            self.sized.push(self.visible.len());
        }
        self.visible.push(Visible {
            shape,
            bytes,
            sized,
            interval_separator: false,
        });
    }
    fn truncate_visible(&mut self, length: usize) {
        self.visible.truncate(length);
        while self.sized.last().is_some_and(|at| *at >= length) {
            self.sized.pop();
        }
    }
    fn close(&mut self, shape: Shape, bytes: Range<usize>, index: &mut TexDelimiterIndex) {
        let floor = self.floor();
        if self.visible.len() <= floor {
            return;
        }
        if self.visible.last().is_some_and(|open| open.sized) {
            return;
        }
        if self.visible.last().is_some_and(|open| open.shape == shape) {
            let open = self.visible.pop().unwrap();
            index.pair(open.bytes, bytes, Some(shape.family()));
        } else if self.in_math()
            && matches!(shape, Shape::Round | Shape::Square)
            && self.visible.last().is_some_and(|open| {
                open.interval_separator && matches!(open.shape, Shape::Round | Shape::Square)
            })
        {
            let open = self.visible.pop().unwrap();
            index.pair(open.bytes, bytes, Some(BracketFamily::Mixed));
        } else {
            let floor = self
                .sized
                .last()
                .filter(|at| **at >= floor)
                .map_or(floor, |at| at + 1);
            self.truncate_visible(floor);
        }
    }
    fn neutral(&mut self, shape: Shape, bytes: Range<usize>, index: &mut TexDelimiterIndex) {
        if self.visible.len() > self.floor()
            && self
                .visible
                .last()
                .is_some_and(|open| open.shape == shape && !open.sized)
        {
            self.close(shape, bytes, index);
        } else {
            self.open(shape, bytes, false);
        }
    }
    fn close_sized(&mut self, shape: Shape, bytes: Range<usize>, index: &mut TexDelimiterIndex) {
        let floor = self.floor();
        if let Some(at) = self.sized.last().copied().filter(|at| *at >= floor) {
            self.truncate_visible(at + 1);
            self.sized.pop();
            let open = self.visible.pop().unwrap();
            let family = if open.shape == shape {
                shape.family()
            } else {
                BracketFamily::Mixed
            };
            index.pair(open.bytes, bytes, Some(family));
        }
    }
    fn math_open(&mut self, kind: Math, bytes: Range<usize>) {
        self.math.push(MathOpen {
            kind,
            bytes,
            visible: self.visible.len(),
            groups: self.groups.len(),
        });
    }
    fn math_marker(
        &mut self,
        kind: Math,
        bytes: Range<usize>,
        toggle: bool,
        index: &mut TexDelimiterIndex,
    ) {
        if self
            .math
            .last()
            .is_some_and(|open| open.kind == kind && open.groups == self.groups.len())
        {
            let open = self.math.pop().unwrap();
            self.truncate_visible(open.visible);
            index.math_regions.push(open.bytes.end..bytes.start);
            let family = match kind {
                Math::Inline => Some(BracketFamily::Round),
                Math::Display => Some(BracketFamily::Square),
                _ => None,
            };
            index.pair(open.bytes, bytes, family);
        } else if toggle && self.math.is_empty() {
            self.math_open(kind, bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_pair(source: &str, left: &str, right: &str) {
        let index = TexDelimiterIndex::new(source);
        let first = source.find(left).unwrap();
        let last = source.rfind(right).unwrap();
        let left_length = if left.starts_with('{') { 1 } else { left.len() };
        let pair = [first..first + left_length, last..last + right.len()];
        for position in [first, first + left_length - 1, last, last + right.len() - 1] {
            assert_eq!(
                index.pair_at(position),
                Some(pair.clone()),
                "{source} at {position}"
            );
        }
    }

    #[test]
    fn tex_groups_math_and_printed_delimiters_keep_utf8_byte_ranges() {
        for (source, left, right) in [
            ("λ \\textbf{café [nested]}", "{", "}"),
            ("text (nested [body])", "(", ")"),
            ("λ $\\{x\\}$", "\\{", "\\}"),
            ("λ $$x^2$$", "$$", "$$"),
            ("λ \\(x^2\\)", "\\(", "\\)"),
            ("λ \\[x^2\\]", "\\[", "\\]"),
            ("$\\left( a + \\left[b\\right] \\right)$", "(", ")"),
            ("$\\left\\langle x\\right\\rangle$", "\\langle", "\\rangle"),
            ("$\\left\\{x\\right.$", "\\{", "."),
        ] {
            assert_pair(source, left, right);
        }
    }

    #[test]
    fn environments_match_whole_named_tokens_including_starred_and_literal_containers() {
        for name in [
            "document",
            "equation*",
            "my-environment",
            "my:env_*",
            "résumé",
            "verbatim",
            "lstlisting",
            "minted",
            "comment",
        ] {
            let open = format!("\\begin{{{name}}}");
            let close = format!("\\end{{{name}}}");
            let source = format!("λ {open}\nPayload\n{close}");
            let index = TexDelimiterIndex::new(&source);
            let left = source.find(&open).unwrap();
            let right = source.rfind(&close).unwrap();
            let pair = [left..left + open.len(), right..right + close.len()];
            assert_eq!(index.pair_at(left), Some(pair.clone()), "{name}");
            assert_eq!(index.pair_at(right + 2), Some(pair.clone()), "{name}");
            let spans: Vec<_> = index.brackets().map(|(range, _, _)| range).collect();
            assert_eq!(
                spans, pair,
                "header braces cannot overlap full environment tokens"
            );
        }
    }

    #[test]
    fn comments_escapes_inline_and_environment_literals_cannot_create_pairs() {
        for source in [
            "% { fake () }\n",
            "\\{escaped\\}",
            "\\verb|{fake}|",
            "\\verb*[fake[",
            "\\lstinline[language=TeX]|{fake}|",
            "\\mintinline{tex}{\\left(x\\right)}",
        ] {
            let index = TexDelimiterIndex::new(source);
            assert!(
                index.endpoints.is_empty(),
                "{source}: {:?}",
                index.endpoints
            );
        }
        assert_pair("\\\\% ignored {fake}\n{real}", "{real", "}");
        for source in [
            "\\begin{Verbatim}{fake}\\end{Verbatim}",
            "\\begin{comment}{fake}\\end{comment}",
        ] {
            let index = TexDelimiterIndex::new(source);
            assert_eq!(
                index.endpoints.len(),
                2,
                "only the outer literal markers pair"
            );
            assert!(index.pair_at(source.find("{fake}").unwrap()).is_none());
        }
    }

    #[test]
    fn environment_nesting_names_and_literal_start_admission_are_exact() {
        let source = "\\begin{outer}α \\begin{align*}x\\end{align*} tail\\end{outer}";
        let index = TexDelimiterIndex::new(source);
        let inner = source.find("\\begin{align*}").unwrap();
        let end = source.find("\\end{align*}").unwrap();
        let expected = Some([
            inner..inner + "\\begin{align*}".len(),
            end..end + "\\end{align*}".len(),
        ]);
        assert_eq!(index.pair_at_caret(source.find('x').unwrap()), expected);
        assert_eq!(
            index.pair_at_caret(source.find("tail").unwrap()),
            index.pair_at(0)
        );
        let depths: Vec<_> = index
            .brackets()
            .map(|(_, family, depth)| (family, depth))
            .collect();
        assert_eq!(
            depths,
            [
                (BracketFamily::Curly, 0),
                (BracketFamily::Curly, 1),
                (BracketFamily::Curly, 1),
                (BracketFamily::Curly, 0)
            ]
        );
        for malformed in [
            "\\begin{a}\\end{a*}",
            "\\begin{a}\\begin{b}\\end{a}\\end{b}",
            "\\begin{a}body",
        ] {
            let index = TexDelimiterIndex::new(malformed);
            assert!(index.pair_at(0).is_none(), "{malformed}");
            assert!(
                index.environment_start_at(0),
                "unmatched real opens remain eligible"
            );
        }
        for name in ["verbatim", "lstlisting", "minted", "comment"] {
            let source = format!(
                "\\begin{{{name}}}\n{{fake}} \\begin{{wrong}}\\end{{wrong}}\n\\end{{{name}}}"
            );
            let index = TexDelimiterIndex::new(&source);
            assert!(index.environment_start_at(0));
            let fake = source.find("\\begin{wrong}").unwrap();
            assert!(!index.environment_start_at(fake));
            assert!(index.ignored_at(fake));
            assert!(index.pair_at(fake).is_none());
            assert_eq!(index.pair_at_caret(fake), index.pair_at(0));
            let incomplete = format!("\\begin{{{name}}}\n{{fake}}");
            let index = TexDelimiterIndex::new(&incomplete);
            assert!(index.environment_start_at(0));
            assert!(index.pair_at(0).is_none());
        }
        for source in [
            "% \\begin{fake}\n",
            "\\verb|\\begin{fake}|",
            "\\url{https://example.test/\\begin{fake}}",
        ] {
            let index = TexDelimiterIndex::new(source);
            assert!(
                !index.environment_start_at(source.find("\\begin").unwrap()),
                "{source}"
            );
        }
    }

    #[test]
    fn caret_matching_selects_nearest_tex_pair_without_changing_endpoint_queries() {
        let source = "λ \\begin{document}outside (inner [body]) tail % [fake]\n\\end{document}";
        let index = TexDelimiterIndex::new(source);
        let inner = source.find("[body]").unwrap();
        let close = inner + "[body".len();
        assert!(
            index.pair_at(inner + 2).is_none(),
            "syntax consumers still query endpoints only"
        );
        assert_eq!(
            index.pair_at_caret(inner + 2),
            Some([inner..inner + 1, close..close + 1])
        );
        let round = source.find('(').unwrap();
        let round_end = source.find(')').unwrap();
        assert_eq!(
            index.pair_at_caret(close + 1),
            Some([round..round + 1, round_end..round_end + 1])
        );
        let begin = source.find("\\begin").unwrap();
        let environment = index.pair_at(begin);
        assert_eq!(
            index.pair_at_caret(source.find("tail").unwrap()),
            environment
        );
        assert_eq!(
            index.pair_at_caret(source.find("fake").unwrap()),
            environment
        );
        assert_eq!(index.pair_at_caret(source.len()), environment);
        assert!(index.pair_at_caret(usize::MAX).is_none());
        assert!(
            TexDelimiterIndex::new("\\begin{a}unclosed")
                .pair_at_caret(12)
                .is_none()
        );
        let source = "{[inner] tail}";
        let index = TexDelimiterIndex::new(source);
        assert_eq!(index.pair_at_caret(8), Some([1..2, 7..8]));
        assert_eq!(index.pair_at_caret(9), Some([0..1, 13..14]));
        assert_eq!(
            index.pair_at_caret(13),
            Some([0..1, 13..14]),
            "token under caret wins over previous adjacency"
        );
        let source = "\\begin % real header\n {my:env*}body\\end{my:env*}";
        let index = TexDelimiterIndex::new(source);
        let close = source.find("\\end").unwrap();
        assert_eq!(
            index.pair_at(0),
            Some([0..source.find("body").unwrap(), close..source.len()])
        );
    }

    #[test]
    fn enter_closer_admission_reserves_same_named_unmatched_ancestors_without_changing_pairs() {
        for (source, expected) in [
            ("\\begin{itemize}\n  \\begin{itemize}\n\\end{itemize}", true),
            (
                "\\begin{itemize}\n  \\begin{itemize}\n\\end{itemize}\n\\end{itemize}",
                false,
            ),
            ("\\begin{outer}\n  \\begin{itemize}\n\\end{itemize}", false),
            (
                "\\begin{itemize}\n  \\begin{itemize*}\n\\end{itemize*}",
                false,
            ),
        ] {
            let start = source.rfind("\\begin").unwrap();
            let index = TexDelimiterIndex::new(source);
            assert!(index.environment_start_at(start));
            assert!(
                index.pair_at(start).is_some(),
                "normal matching remains LIFO"
            );
            assert_eq!(index.environment_needs_closer(start), expected, "{source}");
        }
        for source in ["\\begin{minted}{tex}", "\\begin{custom:name*}"] {
            assert!(TexDelimiterIndex::new(source).environment_needs_closer(0));
        }
        for source in [
            "\\begin{minted}{tex}\nbody\n\\end{minted}",
            "% \\begin{fake}",
        ] {
            let start = source.find("\\begin").unwrap();
            assert!(!TexDelimiterIndex::new(source).environment_needs_closer(start));
        }
    }

    #[test]
    fn malformed_pairs_do_not_cross_or_consume_a_later_valid_group() {
        for source in ["([)]", "(unclosed", "$unclosed", "\\(unclosed", "\\left(x"] {
            assert!(
                TexDelimiterIndex::new(source).endpoints.is_empty(),
                "{source}"
            );
        }
        assert_pair("{(plain unmatched} {good}", "{good", "}");
        assert_pair("% fake { }\n\\verb|[fake]|\n{good}", "{good", "}");
    }

    #[test]
    fn ignored_caret_boundaries_distinguish_closed_and_unclosed_literals() {
        for source in [
            "% unfinished",
            "\\verb|unfinished",
            "\\begin{verbatim} unfinished",
        ] {
            let index = TexDelimiterIndex::new(source);
            assert!(!index.ignored_at(source.len()));
            assert!(index.ignored_at_caret(source.len()), "{source}");
        }
        for source in ["\\verb|closed|", "\\begin{verbatim}closed\\end{verbatim}"] {
            assert!(
                !TexDelimiterIndex::new(source).ignored_at_caret(source.len()),
                "{source}"
            );
        }
        let source = "\\% text {ok}";
        assert!(!TexDelimiterIndex::new(source).ignored_at_caret(2));
    }

    #[test]
    fn palettes_follow_only_complete_pairs_and_each_family_has_its_own_depth() {
        let index = TexDelimiterIndex::new("{(outer [inner])} {sibling}");
        let opens: Vec<_> = index
            .brackets()
            .filter(|(range, _, _)| {
                matches!(
                    &"{(outer [inner])} {sibling}"[range.clone()],
                    "{" | "(" | "["
                )
            })
            .map(|(_, family, depth)| (family, depth))
            .collect();
        assert_eq!(
            opens,
            vec![
                (BracketFamily::Curly, 0),
                (BracketFamily::Round, 0),
                (BracketFamily::Square, 0),
                (BracketFamily::Curly, 0)
            ]
        );
        let index = TexDelimiterIndex::new("{{inner}");
        assert!(index.brackets().all(|(_, _, depth)| depth == 0));
        assert_eq!(index.ignored_ranges().len(), 0);
        assert_eq!(tex_ignored_ranges("% one\n\\verb|two|").len(), 2);
    }

    #[test]
    fn math_caret_context_keeps_unclosed_markers_but_not_comments_or_literal_payloads() {
        for source in ["$x", "$$x", "\\(x", "\\[x", "\\begin{equation}x"] {
            assert!(
                TexDelimiterIndex::new(source).math_at_caret(source.len()),
                "{source}"
            );
        }
        for source in [
            "$x$",
            "$$x$$",
            "\\(x\\)",
            "\\[x\\]",
            "\\begin{equation}x\\end{equation}",
        ] {
            assert!(
                !TexDelimiterIndex::new(source).math_at_caret(source.len()),
                "{source}"
            );
        }
        let source = "$a % comment\n\\verb|$| x$";
        let index = TexDelimiterIndex::new(source);
        assert!(!index.math_at_caret(source.find("comment").unwrap()));
        assert!(!index.math_at_caret(source.find("\\verb").unwrap() + 6));
        assert!(index.math_at_caret(source.rfind(" x").unwrap() + 1));
    }

    #[test]
    fn url_payloads_are_literal_without_hiding_outer_groups_or_href_labels() {
        for command in ["url", "nolinkurl", "path", "href"] {
            let source = format!("\\{command}{{https://example.test/a%20(b)[c]}}");
            let index = TexDelimiterIndex::new(&source);
            let open = source.find('{').unwrap();
            let close = source.rfind('}').unwrap();
            assert_eq!(
                index.pair_at(open),
                Some([open..open + 1, close..close + 1])
            );
            for marker in ['%', '(', '['] {
                let at = source.find(marker).unwrap();
                assert!(index.ignored_at(at), "{source}");
                assert!(index.pair_at(at).is_none(), "{source}");
            }
            assert!(index.ignored_at_caret(close));
            assert!(!index.ignored_at(close));
        }
        let source = "\\href{https://example.test/a%20b}{(visible label)}";
        let index = TexDelimiterIndex::new(source);
        let open = source.find('(').unwrap();
        let close = source.find(')').unwrap();
        assert_eq!(
            index.pair_at(open),
            Some([open..open + 1, close..close + 1])
        );
        let source = "\\url % {fake}\n{https://example.test/a%20b}";
        assert!(
            TexDelimiterIndex::new(source)
                .pair_at(source.find("{fake").unwrap())
                .is_none()
        );
    }

    #[test]
    fn middle_separators_do_not_pair_with_each_other_as_brackets() {
        let source = "$\\left\\{x\\middle|p\\middle|q\\right\\}$";
        let index = TexDelimiterIndex::new(source);
        assert_eq!(index.brackets().count(), 2);
        for (at, _) in source.match_indices('|') {
            assert!(index.pair_at(at).is_none());
        }
    }

    #[test]
    fn bare_mixed_math_intervals_require_a_same_level_separator() {
        for source in ["$(a,b]$", "$[a;b)$", "$(\\text{start},f(x,y)]$"] {
            let index = TexDelimiterIndex::new(source);
            let left = source.find(['(', '[']).unwrap();
            let right = source.rfind([')', ']']).unwrap();
            assert_eq!(
                index.pair_at(left),
                Some([left..left + 1, right..right + 1]),
                "{source}"
            );
            assert!(index.brackets().any(|(range, family, _)| range.start == left && family == BracketFamily::Mixed));
        }
        for source in ["(a,b]", "$([)]$", "$[(])$"] {
            assert!(
                TexDelimiterIndex::new(source).brackets().next().is_none(),
                "{source}"
            );
        }
        let source = "$ (f(x,y)] $";
        let index = TexDelimiterIndex::new(source);
        assert!(
            index.pair_at(source.find('(').unwrap()).is_none(),
            "an inner call's comma cannot legitimize an outer mismatch"
        );
    }

    #[test]
    fn href_options_keep_literal_percent_and_escaped_header_percent_distinct() {
        let source = "\\href[pdfnewwindow=true,color={red},label=\\%] % {comment}\n{https://example.test/a%20b#part}{Label}";
        let index = TexDelimiterIndex::new(source);
        let open = source.find("{https").unwrap();
        let close = source[open..].find('}').unwrap() + open;
        assert_eq!(
            index.pair_at(open),
            Some([open..open + 1, close..close + 1])
        );
        assert!(!index.ignored_at(source.find("\\%").unwrap() + 1));
        assert!(index.ignored_at(source.find("%20").unwrap()));
        assert!(index.pair_at(source.find("{comment}").unwrap()).is_none());
        let option = source.find('[').unwrap();
        assert!(index.pair_at(option).is_some());
    }
}
