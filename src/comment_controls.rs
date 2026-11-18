//! Configurable controls expressed only by standalone source comments.
use crate::document::DocumentKind;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use typst_syntax::{Source, SyntaxKind, SyntaxNode};

pub(crate) const MAX_MARKER_BYTES: usize = 128;

/// Comment bodies, without the language's `%` or `//` prefix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawMarkers")]
pub(crate) struct Markers {
    pub(crate) off: String,
    pub(crate) on: String,
}
impl Default for Markers {
    fn default() -> Self {
        Self {
            off: "ttt: off".into(),
            on: "ttt: on".into(),
        }
    }
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawMarkers {
    off: String,
    on: String,
}
impl Default for RawMarkers {
    fn default() -> Self {
        let markers = Markers::default();
        Self {
            off: markers.off,
            on: markers.on,
        }
    }
}
impl TryFrom<RawMarkers> for Markers {
    type Error = String;
    fn try_from(value: RawMarkers) -> Result<Self, Self::Error> {
        let markers = Self {
            off: value.off,
            on: value.on,
        };
        markers.validate()?;
        Ok(markers)
    }
}
/// Marker payload comparison ignores Unicode whitespace without changing case.
/// The lexical reader still decides whether the payload is a standalone comment.
pub(crate) fn marker_matches(left: &str, right: &str) -> bool {
    left.chars()
        .filter(|ch| !ch.is_whitespace())
        .eq(right.chars().filter(|ch| !ch.is_whitespace()))
}

impl Markers {
    pub(crate) fn validate(&self) -> Result<(), String> {
        for (name, value) in [("off", &self.off), ("on", &self.on)] {
            if value.len() > MAX_MARKER_BYTES {
                return Err(format!("{name}: use at most {MAX_MARKER_BYTES} bytes"));
            }
            if !value.chars().any(|ch| !ch.is_whitespace()) {
                return Err(format!(
                    "{name}: enter marker text containing a visible character"
                ));
            }
            if value.chars().any(|ch| {
                matches!(ch, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
                    || ch.is_control() && ch != '\t'
            }) {
                return Err(format!("{name}: use one line without control characters"));
            }
            let mut payload = value.chars().filter(|ch| !ch.is_whitespace());
            let first = payload.next();
            if first == Some('%') || first == Some('/') && matches!(payload.next(), Some('/' | '*'))
            {
                return Err(format!(
                    "{name}: omit the comment prefix; use only the marker text"
                ));
            }
        }
        if marker_matches(&self.off, &self.on) {
            return Err("off and on markers must be different after ignoring whitespace".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Feature {
    Formatting,
    Diagnostics,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawSettings")]
pub(crate) struct Settings {
    pub(crate) both: Markers,
    pub(crate) formatting: Markers,
    pub(crate) diagnostics: Markers,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            both: Markers::default(),
            formatting: Markers {
                off: "fmt: off".into(),
                on: "fmt: on".into(),
            },
            diagnostics: Markers {
                off: "diag: off".into(),
                on: "diag: on".into(),
            },
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct MarkerOverrides {
    #[serde(default, deserialize_with = "marker_override")]
    off: Option<String>,
    #[serde(default, deserialize_with = "marker_override")]
    on: Option<String>,
}
fn marker_override<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawSettings {
    both: MarkerOverrides,
    formatting: MarkerOverrides,
    diagnostics: MarkerOverrides,
}
impl TryFrom<RawSettings> for Settings {
    type Error = String;
    fn try_from(value: RawSettings) -> Result<Self, Self::Error> {
        let mut settings = Self::default();
        for (markers, overrides) in [
            (&mut settings.both, value.both),
            (&mut settings.formatting, value.formatting),
            (&mut settings.diagnostics, value.diagnostics),
        ] {
            if let Some(off) = overrides.off {
                markers.off = off;
            }
            if let Some(on) = overrides.on {
                markers.on = on;
            }
        }
        settings.validate()?;
        Ok(settings)
    }
}
impl Settings {
    pub(crate) fn validate(&self) -> Result<(), String> {
        for (name, markers) in [
            ("both", &self.both),
            ("formatting", &self.formatting),
            ("diagnostics", &self.diagnostics),
        ] {
            markers
                .validate()
                .map_err(|error| format!("{name}.{error}"))?;
        }
        let values = [
            ("both.off", &self.both.off),
            ("both.on", &self.both.on),
            ("formatting.off", &self.formatting.off),
            ("formatting.on", &self.formatting.on),
            ("diagnostics.off", &self.diagnostics.off),
            ("diagnostics.on", &self.diagnostics.on),
        ];
        for (index, (name, value)) in values.iter().enumerate() {
            if let Some((other, _)) = values[..index]
                .iter()
                .find(|(_, earlier)| marker_matches(earlier, value))
            {
                return Err(format!("{name}: marker must differ from {other}"));
            }
        }
        Ok(())
    }
    fn pair(&self, feature: Feature) -> &Markers {
        match feature {
            Feature::Formatting => &self.formatting,
            Feature::Diagnostics => &self.diagnostics,
        }
    }
    /// Bounded KMP states scan one Unicode source stream, skipping whitespace.
    /// The settings limit bounds all six stack-resident patterns; no normalized
    /// document copy or per-marker scan/allocation is needed on the fast path.
    pub(crate) fn occurs_in(&self, source: &str) -> bool {
        let values = [
            self.both.off.as_str(),
            self.both.on.as_str(),
            self.formatting.off.as_str(),
            self.formatting.on.as_str(),
            self.diagnostics.off.as_str(),
            self.diagnostics.on.as_str(),
        ];
        if values.iter().any(|value| {
            value.len() > MAX_MARKER_BYTES || !value.chars().any(|ch| !ch.is_whitespace())
        }) {
            // Invalid public drafts must not produce a false negative. parse()
            // rejects them before any lexical reading or region publication.
            return true;
        }
        let mut patterns = values.map(Candidate::new);
        for ch in source.chars().filter(|ch| !ch.is_whitespace()) {
            for pattern in &mut patterns {
                if pattern.advance(ch) {
                    return true;
                }
            }
        }
        false
    }
    fn recognizes(&self, body: &str) -> bool {
        [&self.both, &self.formatting, &self.diagnostics]
            .iter()
            .any(|pair| marker_matches(body, &pair.off) || marker_matches(body, &pair.on))
    }
}

/// Prefix-function matching gives linear work even for long repeated prefixes.
/// Each valid marker has at most MAX_MARKER_BYTES Unicode scalar values.
struct Candidate {
    characters: [char; MAX_MARKER_BYTES],
    fallback: [usize; MAX_MARKER_BYTES],
    len: usize,
    matched: usize,
}
impl Candidate {
    fn new(value: &str) -> Self {
        let mut candidate = Self {
            characters: ['\0'; MAX_MARKER_BYTES],
            fallback: [0; MAX_MARKER_BYTES],
            len: 0,
            matched: 0,
        };
        for ch in value.chars().filter(|ch| !ch.is_whitespace()) {
            candidate.characters[candidate.len] = ch;
            candidate.len += 1;
        }
        for at in 1..candidate.len {
            let mut prefix = candidate.fallback[at - 1];
            while prefix > 0 && candidate.characters[at] != candidate.characters[prefix] {
                prefix = candidate.fallback[prefix - 1];
            }
            if candidate.characters[at] == candidate.characters[prefix] {
                prefix += 1;
            }
            candidate.fallback[at] = prefix;
        }
        candidate
    }
    fn advance(&mut self, ch: char) -> bool {
        while self.matched > 0 && self.characters[self.matched] != ch {
            self.matched = self.fallback[self.matched - 1];
        }
        if self.characters[self.matched] == ch {
            self.matched += 1;
        }
        if self.matched == self.len {
            self.matched = self.fallback[self.len - 1];
            true
        } else {
            false
        }
    }
}

/// Disabled interior lines, control metadata lines and protected source bytes.
/// Cache with the source revision and all markers, never a provider publication.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Regions {
    lines: Vec<Range<usize>>,
    marker_lines: Vec<usize>,
    protected: Vec<Range<usize>>,
}
impl Regions {
    pub(crate) fn parse(
        source: &str,
        kind: DocumentKind,
        settings: &Settings,
        feature: Feature,
    ) -> Self {
        // Most documents have no directives. Skip syntax work and allocations.
        if settings.validate().is_err() || !settings.occurs_in(source) {
            return Self::default();
        }
        let comments = match kind {
            DocumentKind::Typst => typst_comments(source),
            DocumentKind::Tex => tex_comments(source),
            _ => return Self::default(),
        };
        let markers = settings.pair(feature);
        let formatting = feature == Feature::Formatting;
        let mut result = Self::default();
        let mut excluded_from = None;
        let mut line = 1;
        let mut previous = 0;
        for comment in comments {
            line += source[previous..comment.start]
                .bytes()
                .filter(|&byte| byte == b'\n')
                .count();
            previous = comment.start;
            let line_start = source[..comment.start].rfind('\n').map_or(0, |at| at + 1);
            if !source[line_start..comment.start].trim().is_empty() {
                continue;
            }
            let prefix = if kind == DocumentKind::Typst { 2 } else { 1 };
            let body = source[comment.start + prefix..comment.end].trim();
            let recognized = settings.recognizes(body);
            if recognized && result.marker_lines.last() != Some(&line) {
                result.marker_lines.push(line);
            }
            let line_end = source[comment.start..]
                .find('\n')
                .map_or(source.len(), |at| comment.start + at + 1);
            if marker_matches(body, &settings.both.off) || marker_matches(body, &markers.off) {
                excluded_from.get_or_insert((line + 1, line_start));
            } else if marker_matches(body, &settings.both.on) || marker_matches(body, &markers.on) {
                if let Some((start, bytes)) = excluded_from.take() {
                    if start < line {
                        result.lines.push(start..line);
                    }
                    if formatting {
                        result.protect(bytes..line_end);
                    }
                } else if formatting {
                    result.protect(line_start..line_end);
                }
            } else if formatting && excluded_from.is_none() && recognized {
                // Formatting never rewrites any control marker, including a
                // diagnostics-only marker surrounding otherwise editable text.
                result.protect(line_start..line_end);
            }
        }
        if let Some((start, bytes)) = excluded_from {
            let end = line
                + source[previous..]
                    .bytes()
                    .filter(|&byte| byte == b'\n')
                    .count()
                + 1;
            if start < end {
                result.lines.push(start..end);
            }
            if formatting {
                result.protect(bytes..source.len());
            }
        }
        result
    }
    fn protect(&mut self, range: Range<usize>) {
        if let Some(previous) = self.protected.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            self.protected.push(range);
        }
    }
    pub(crate) fn contains_line(&self, line: usize) -> bool {
        let after = self.lines.partition_point(|range| range.start <= line);
        after > 0 && self.lines[after - 1].contains(&line)
    }
    pub(crate) fn contains_marker_line(&self, line: usize) -> bool {
        self.marker_lines.binary_search(&line).is_ok()
    }
    pub(crate) fn protected_ranges(&self) -> &[Range<usize>] {
        &self.protected
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.marker_lines.is_empty() && self.protected.is_empty()
    }
}

fn typst_comments(source: &str) -> Vec<Range<usize>> {
    fn visit(node: &SyntaxNode, offset: usize, comments: &mut Vec<Range<usize>>) {
        match node.kind() {
            SyntaxKind::LineComment => comments.push(offset..offset + node.len()),
            SyntaxKind::Str | SyntaxKind::Raw | SyntaxKind::BlockComment => {}
            _ => {
                let mut offset = offset;
                for child in node.children() {
                    visit(child, offset, comments);
                    offset += child.len();
                }
            }
        }
    }
    let parsed = Source::detached(source);
    let mut comments = Vec::new();
    visit(parsed.root(), 0, &mut comments);
    comments
}

/// Tolerant TeX lexical reading, without executing macros or changing catcodes.
/// Shared inline-literal recognition keeps samples from creating directives.
fn tex_comments(source: &str) -> Vec<Range<usize>> {
    let mut comments = Vec::new();
    let mut at = 0;
    while at < source.len() {
        match source.as_bytes()[at] {
            b'%' => {
                let end = at + source[at..].find('\n').unwrap_or(source.len() - at);
                comments.push(at..end);
                at = end;
            }
            b'\\' => {
                if let Some(literal) = crate::editor_math_guides::tex_inline_literal(source, at) {
                    // Header whitespace/options/language arguments still use
                    // ordinary TeX comments; only the literal payload changes
                    // their meaning. Keep extraction within the shared span.
                    tex_inline_header_comments(&source[..literal.end], at, &mut comments);
                    at = literal.end;
                    continue;
                }
                let start = at + 1;
                at = start;
                while source
                    .as_bytes()
                    .get(at)
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'@')
                {
                    at += 1;
                }
                if at == start {
                    at += source[at..].chars().next().map_or(0, char::len_utf8);
                    continue;
                }
                if &source[start..at] == "begin" {
                    let argument = tex_header_space(source, at, &mut comments);
                    if source[argument..].starts_with('{')
                        && let Some(end) =
                            source[argument + 1..].find(['}', '{', '\\', '%', '\n', '\r'])
                    {
                        let end = argument + 1 + end;
                        if source.as_bytes()[end] == b'}' {
                            let environment = source[argument + 1..end].trim();
                            if matches!(
                                environment,
                                "verbatim"
                                    | "verbatim*"
                                    | "Verbatim"
                                    | "BVerbatim"
                                    | "LVerbatim"
                                    | "SaveVerbatim"
                                    | "VerbatimOut"
                                    | "lstlisting"
                                    | "minted"
                                    | "comment"
                                    | "filecontents"
                                    | "filecontents*"
                            ) {
                                at = end + 1;
                                if environment == "comment" {
                                    at = tex_comment_environment_end(source, at);
                                } else {
                                    let closing = format!("\\end{{{environment}}}");
                                    at += source[at..]
                                        .find(&closing)
                                        .map_or(source.len() - at, |offset| offset + closing.len());
                                }
                            } else {
                                at = end + 1;
                            }
                        } else {
                            at = argument;
                        }
                    } else {
                        at = argument;
                    }
                }
            }
            _ => at += source[at..].chars().next().unwrap().len_utf8(),
        }
    }
    comments
}

/// comment.sty's ProcessCommentLine compares a whole line to EndCommentTest.
/// An inline example, indented marker or trailing text cannot end its payload.
fn tex_comment_environment_end(source: &str, start: usize) -> usize {
    let closing = "\\end{comment}";
    // ProcessComment discards the remainder of the opening command's line.
    let mut at = start
        + source[start..]
            .find('\n')
            .map_or(source.len() - start, |offset| offset + 1);
    while at < source.len() {
        let end = at + source[at..].find('\n').unwrap_or(source.len() - at);
        let line = source[at..end]
            .strip_suffix('\r')
            .unwrap_or(&source[at..end]);
        if line == closing {
            return at + closing.len();
        }
        at = (end + 1).min(source.len());
    }
    source.len()
}

fn tex_inline_header_comments(source: &str, start: usize, comments: &mut Vec<Range<usize>>) {
    let mut at = start + 1;
    while source
        .as_bytes()
        .get(at)
        .is_some_and(u8::is_ascii_alphabetic)
    {
        at += 1;
    }
    let name = &source[start + 1..at];
    if !matches!(name, "lstinline" | "mintinline") {
        return;
    }
    at = tex_header_space(source, at, comments);
    if source[at..].starts_with('[') {
        let Some(end) = tex_header_group_end(source, at, b'[', b']', comments) else {
            return;
        };
        at = tex_header_space(source, end, comments);
    }
    if name == "mintinline" && source[at..].starts_with('{') {
        let _ = tex_header_group_end(source, at, b'{', b'}', comments);
        // After the language argument, '%' may itself be the literal
        // delimiter. Do not continue searching for comments into the payload.
    }
}

fn tex_header_group_end(
    source: &str,
    start: usize,
    open: u8,
    close: u8,
    comments: &mut Vec<Range<usize>>,
) -> Option<usize> {
    let mut at = start + 1;
    let mut depth = 1;
    let mut braces = 0_usize;
    while at < source.len() {
        let byte = source.as_bytes()[at];
        if byte == b'%' {
            let end = at + source[at..].find('\n').unwrap_or(source.len() - at);
            comments.push(at..end);
            at = end;
            continue;
        }
        if byte == b'\\' {
            at += 1;
            at += source[at..].chars().next().map_or(0, char::len_utf8);
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
                return Some(at + 1);
            }
        }
        at += source[at..].chars().next().unwrap().len_utf8();
    }
    None
}

fn tex_header_space(source: &str, mut at: usize, comments: &mut Vec<Range<usize>>) -> usize {
    loop {
        while let Some(character) = source[at..].chars().next().filter(|ch| ch.is_whitespace()) {
            at += character.len_utf8();
        }
        if source[at..].starts_with('%') {
            let end = at + source[at..].find('\n').unwrap_or(source.len() - at);
            comments.push(at..end);
            at = end;
        } else {
            return at;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, kind: DocumentKind) -> Regions {
        Regions::parse(source, kind, &Settings::default(), Feature::Diagnostics)
    }

    #[test]
    fn custom_markers_have_exact_body_matches_and_preserve_utf8_crlf_boundaries() {
        let markers = Markers {
            off: "pause ✋".into(),
            on: "resume ✅".into(),
        };
        for (prefix, kind) in [("%", DocumentKind::Tex), ("//", DocumentKind::Typst)] {
            let source = format!(
                "Before α\r\n  {prefix}pause ✋ \r\n  raw 😀  \r\n{prefix}resume ✅\r\nAfter β"
            );
            let settings = Settings {
                both: markers.clone(),
                ..Default::default()
            };
            let regions = Regions::parse(&source, kind, &settings, Feature::Formatting);
            assert!(!regions.contains_line(2));
            assert!(regions.contains_line(3));
            assert!(!regions.contains_line(4));
            let start = source.find("  ").unwrap();
            let end = source.find("After").unwrap();
            assert_eq!(
                regions.protected_ranges(),
                std::slice::from_ref(&(start..end))
            );
            assert!(source.is_char_boundary(start) && source.is_char_boundary(end));
            let default_text = format!("{prefix}ttt: off\nnot disabled\n{prefix}ttt: on");
            assert!(
                Regions::parse(&default_text, kind, &settings, Feature::Diagnostics).is_empty()
            );
            let almost = format!("{prefix}pause ✋ extra\nnot disabled\n{prefix}resume ✅");
            assert!(
                !Regions::parse(&almost, kind, &settings, Feature::Diagnostics).contains_line(2)
            );
        }
    }

    #[test]
    fn recognized_control_lines_are_protected_even_outside_a_region() {
        let markers = Markers {
            off: "stop".into(),
            on: "resume".into(),
        };
        let source = "before\n%resume\nx\n%stop\ny\n%stop\nz\n%resume\nafter";
        let settings = Settings {
            both: markers,
            ..Default::default()
        };
        let regions = Regions::parse(source, DocumentKind::Tex, &settings, Feature::Formatting);
        assert_eq!(regions.lines.as_slice(), std::slice::from_ref(&(5..8)));
        let first_start = source.find("%resume").unwrap();
        let first_end = source.find("x\n").unwrap();
        let second_start = source.find("%stop").unwrap();
        let second_end = source.find("after").unwrap();
        assert_eq!(
            regions.protected_ranges(),
            [first_start..first_end, second_start..second_end]
        );
        for source in ["%ttt: off", "%ttt: on", "%ttt: off\r\n%ttt: on"] {
            let regions = Regions::parse(
                source,
                DocumentKind::Tex,
                &Settings::default(),
                Feature::Formatting,
            );
            assert_eq!(
                regions.protected_ranges(),
                std::slice::from_ref(&(0..source.len()))
            );
            assert!(regions.lines.is_empty());
        }
    }

    #[test]
    fn unmatched_off_protects_through_eof_and_same_channel_switches_do_not_nest() {
        let source = "%ttt: off\nfirst\n%ttt: off\nsecond\n%ttt: on\nvisible\n%ttt: off\nlast\n";
        let regions = Regions::parse(
            source,
            DocumentKind::Tex,
            &Settings::default(),
            Feature::Formatting,
        );
        assert_eq!(regions.lines, [2..5, 8..10]);
        let first_end = source.find("visible").unwrap();
        let last_start = source.rfind("%ttt: off").unwrap();
        assert_eq!(
            regions.protected_ranges(),
            [0..first_end, last_start..source.len()]
        );
        assert!(!regions.contains_line(6));
        assert!(regions.contains_line(9));
        assert!(!regions.contains_line(10));
    }

    #[test]
    fn formatting_and_diagnostics_can_use_independent_pairs() {
        let settings = Settings {
            both: Markers::default(),
            formatting: Markers {
                off: "fmt: stop".into(),
                on: "fmt: resume".into(),
            },
            diagnostics: Markers {
                off: "checks: stop".into(),
                on: "checks: resume".into(),
            },
        };
        let source = "%fmt: stop\nraw\n%fmt: resume\n%checks: stop\ntypo\n%checks: resume";
        let formatting = Regions::parse(source, DocumentKind::Tex, &settings, Feature::Formatting);
        let diagnostics =
            Regions::parse(source, DocumentKind::Tex, &settings, Feature::Diagnostics);
        assert!(formatting.contains_line(2));
        assert!(!formatting.contains_line(5));
        assert!(!diagnostics.contains_line(2));
        assert!(diagnostics.contains_line(5));
    }

    #[test]
    fn common_and_feature_switches_share_one_flat_state_per_feature() {
        let settings = Settings::default();
        let source = "%ttt: off\nboth disabled\n%fmt: on\ndiagnostics only\n%diag: on\nboth enabled\n%fmt: off\nformatting only\n%ttt: on\nboth enabled";
        let formatting = Regions::parse(source, DocumentKind::Tex, &settings, Feature::Formatting);
        let diagnostics =
            Regions::parse(source, DocumentKind::Tex, &settings, Feature::Diagnostics);
        assert_eq!(formatting.lines, [2..3, 8..9]);
        assert_eq!(diagnostics.lines.as_slice(), std::slice::from_ref(&(2..5)));
        assert!(diagnostics.protected_ranges().is_empty());
        assert_eq!(
            formatting
                .protected_ranges()
                .iter()
                .map(|range| &source[range.clone()])
                .collect::<Vec<_>>(),
            [
                "%ttt: off\nboth disabled\n%fmt: on\n",
                "%diag: on\n",
                "%fmt: off\nformatting only\n%ttt: on\n"
            ]
        );
    }

    #[test]
    fn formatting_protects_diagnostics_marker_lines_but_not_their_body() {
        for (prefix, kind) in [("%", DocumentKind::Tex), ("//", DocumentKind::Typst)] {
            let source = format!("{prefix}diag: off\n  editable  \n{prefix}diag: on\nvisible");
            let formatting =
                Regions::parse(&source, kind, &Settings::default(), Feature::Formatting);
            let diagnostics =
                Regions::parse(&source, kind, &Settings::default(), Feature::Diagnostics);
            assert!(formatting.lines.is_empty());
            assert_eq!(diagnostics.lines.as_slice(), std::slice::from_ref(&(2..3)));
            assert_eq!(formatting.protected_ranges().len(), 2);
            for range in formatting.protected_ranges() {
                let protected = &source[range.clone()];
                assert!(protected.starts_with(prefix) && protected.ends_with('\n'));
                assert!(!protected.contains("editable"));
            }
            let nested = format!(
                "{prefix}fmt: off\n{prefix}diag: off\nraw\n{prefix}diag: on\nstill raw\n{prefix}fmt: on\nvisible"
            );
            let protected =
                Regions::parse(&nested, kind, &Settings::default(), Feature::Formatting);
            assert_eq!(
                protected.protected_ranges(),
                std::slice::from_ref(&(0..nested.find("visible").unwrap()))
            );
            assert_eq!(protected.lines.as_slice(), std::slice::from_ref(&(2..6)));
        }
    }

    #[test]
    fn every_control_marker_is_metadata_for_both_features_even_without_a_disabled_body() {
        let defaults = Settings::default();
        let custom = Settings {
            both: Markers {
                off: "all ✋".into(),
                on: "all ✅".into(),
            },
            formatting: Markers {
                off: "format ✋".into(),
                on: "format ✅".into(),
            },
            diagnostics: Markers {
                off: "checks ✋".into(),
                on: "checks ✅".into(),
            },
        };
        for settings in [&defaults, &custom] {
            for (prefix, kind) in [("%", DocumentKind::Tex), ("//", DocumentKind::Typst)] {
                let source = [
                    &settings.both.off,
                    &settings.both.on,
                    &settings.formatting.off,
                    &settings.formatting.on,
                    &settings.diagnostics.off,
                    &settings.diagnostics.on,
                ]
                .map(|body| format!(" {prefix} {body} "))
                .join("\r\n");
                assert!(settings.occurs_in(&source));
                for feature in [Feature::Formatting, Feature::Diagnostics] {
                    let parsed = Regions::parse(&source, kind, settings, feature);
                    assert_eq!(parsed.marker_lines, [1, 2, 3, 4, 5, 6]);
                    assert!(!parsed.is_empty());
                    for line in 1..=6 {
                        assert!(parsed.contains_marker_line(line));
                        assert!(!parsed.contains_line(line));
                    }
                    assert!(!parsed.contains_marker_line(0));
                    assert!(!parsed.contains_marker_line(7));
                    assert!(!parsed.contains_marker_line(usize::MAX));
                }
            }
        }
        // Formatting controls are metadata even while diagnostics remain on.
        let parsed = Regions::parse(
            "%fmt: off\nvisible diagnostic\n%fmt: on",
            DocumentKind::Tex,
            &defaults,
            Feature::Diagnostics,
        );
        assert!(parsed.contains_marker_line(1) && parsed.contains_marker_line(3));
        assert!(!parsed.contains_line(2));
        assert!(!parsed.contains_marker_line(2));
    }

    #[test]
    fn literals_code_and_nonstandalone_mentions_never_become_marker_metadata() {
        let settings = Settings::default();
        let samples = [
            (DocumentKind::Typst, "#let example = \"//fmt: off\""),
            (DocumentKind::Typst, "```typ\n//diag: off\n//ttt: on\n```"),
            (DocumentKind::Typst, "/*\n//fmt: on\n*/"),
            (DocumentKind::Typst, "body //diag: on"),
            (DocumentKind::Typst, "//fmt: off followed by an explanation"),
            (DocumentKind::Tex, "\\verb|%fmt: off|"),
            (
                DocumentKind::Tex,
                "\\begin{verbatim}\n%diag: off\n%ttt: on\n\\end{verbatim}",
            ),
            (DocumentKind::Tex, "\\%fmt: off"),
            (DocumentKind::Tex, "body %diag: on"),
            (DocumentKind::Tex, "%fmt: off followed by an explanation"),
            (DocumentKind::Text, "%ttt: off\n//fmt: off"),
        ];
        for (kind, source) in samples {
            assert!(settings.occurs_in(source));
            for feature in [Feature::Formatting, Feature::Diagnostics] {
                let parsed = Regions::parse(source, kind, &settings, feature);
                assert!(parsed.is_empty(), "{source}");
                assert!(parsed.marker_lines.is_empty(), "{source}");
            }
        }
    }

    #[test]
    fn whitespace_variants_match_markers_without_changing_lexical_or_channel_rules() {
        let settings = Settings::default();
        for (prefix, kind) in [("%", DocumentKind::Tex), ("//", DocumentKind::Typst)] {
            for (off, on, channel) in [
                ("ttt:off", " t t t : o n ", None),
                (" t\tt\u{2003}t : o\u{a0}f f ", "ttt:on", None),
                ("f m t : o f f", "fmt:on", Some(Feature::Formatting)),
                (
                    " d\u{2009}i a g : o f f",
                    "diag:on",
                    Some(Feature::Diagnostics),
                ),
            ] {
                let source = format!("{prefix}{off}\nbody\n{prefix}{on}\nvisible");
                assert!(settings.occurs_in(&source), "candidate missing {source:?}");
                for feature in [Feature::Formatting, Feature::Diagnostics] {
                    let parsed = Regions::parse(&source, kind, &settings, feature);
                    assert!(
                        parsed.contains_marker_line(1) && parsed.contains_marker_line(3),
                        "{source:?}"
                    );
                    assert_eq!(
                        parsed.contains_line(2),
                        channel.is_none_or(|expected| expected == feature),
                        "{source:?}"
                    );
                    assert!(!parsed.contains_line(4));
                }
                let inline = format!("code {prefix}{off}\nbody");
                assert!(Regions::parse(&inline, kind, &settings, Feature::Diagnostics).is_empty());
                let literal = if kind == DocumentKind::Typst {
                    format!("```typ\n{prefix}{off}\n```\nvisible")
                } else {
                    format!("\\begin{{verbatim}}\n{prefix}{off}\n\\end{{verbatim}}\nvisible")
                };
                assert!(Regions::parse(&literal, kind, &settings, Feature::Diagnostics).is_empty());
            }
        }
    }

    #[test]
    fn whitespace_in_custom_markers_is_ignored_and_normalized_duplicates_are_rejected() {
        let settings = Settings {
            both: Markers {
                off: "  block \t: pause ".into(),
                on: "\u{2003}block: resume\u{a0}".into(),
            },
            ..Default::default()
        };
        assert!(settings.validate().is_ok());
        let source = "%b l o c k: p a u s e\nbody\n%block:resume";
        let parsed = Regions::parse(source, DocumentKind::Tex, &settings, Feature::Diagnostics);
        assert!(parsed.contains_line(2));
        assert!(parsed.contains_marker_line(1) && parsed.contains_marker_line(3));
        for (left, right) in [
            ("ttt:off", "ttt: off"),
            ("t\u{2003}t t : o f f", "ttt: off"),
        ] {
            let mut duplicate = Settings::default();
            duplicate.both.off = right.into();
            duplicate.formatting.on = left.into();
            assert!(duplicate.validate().is_err(), "{left:?} vs {right:?}");
            let pair = Markers {
                off: left.into(),
                on: right.into(),
            };
            assert!(pair.validate().is_err());
        }
    }

    #[test]
    fn whitespace_candidate_matching_handles_unicode_long_overlap_and_partial_prefixes() {
        let settings = Settings {
            both: Markers {
                off: "ababababac".into(),
                on: "complete ✅".into(),
            },
            ..Default::default()
        };
        let mut source = "ab ".repeat(4_000);
        assert!(!settings.occurs_in(&source));
        source.push_str("a\u{2003}b\ta\u{a0}b a b a b a c");
        assert!(settings.occurs_in(&source));
        assert!(settings.occurs_in("c o m p l e t e\u{202f}✅"));
        assert!(!settings.occurs_in("complete ❌"));
    }

    #[test]
    fn global_marker_validation_rejects_every_cross_channel_collision() {
        let defaults = Settings::default();
        assert_eq!(defaults.both, Markers::default());
        assert_eq!(defaults.formatting.off, "fmt: off");
        assert_eq!(defaults.diagnostics.off, "diag: off");
        for channel in ["both", "formatting", "diagnostics"] {
            for field in ["off", "on"] {
                let mut value = serde_json::to_value(&defaults).unwrap();
                let original = value[channel][field].clone();
                for other in ["both", "formatting", "diagnostics"] {
                    for other_field in ["off", "on"] {
                        if channel == other && field == other_field {
                            continue;
                        }
                        value[channel][field] = value[other][other_field].clone();
                        assert!(
                            serde_json::from_value::<Settings>(value.clone()).is_err(),
                            "{channel}.{field} vs {other}.{other_field}"
                        );
                        value[channel][field] = original.clone();
                    }
                }
            }
        }
        let partial: Settings = serde_json::from_value(serde_json::json!({
            "both":{"off":"all: stop"}, "formatting":{"off":"fmt: stop"}, "diagnostics":{"off":"checks: stop"}
        })).unwrap();
        assert_eq!(partial.both.on, "ttt: on");
        assert_eq!(partial.formatting.on, "fmt: on");
        assert_eq!(partial.diagnostics.on, "diag: on");
    }

    #[test]
    fn marker_validation_rejects_ambiguous_or_unusable_comment_bodies() {
        for invalid in [
            "",
            "  ",
            "\t\u{2003}\u{a0}",
            "%ttt: off",
            "//ttt: off",
            "/*off",
            "a\nb",
            "a\rb",
            "a\0b",
            " \t%ttt: off",
            "/ \t/ttt: off",
            "a\u{b}b",
            "a\u{c}b",
            "a\u{2028}b",
            "a\u{2029}b",
        ] {
            let markers = Markers {
                off: invalid.into(),
                ..Default::default()
            };
            assert!(markers.validate().is_err(), "{invalid:?}");
            assert!(
                Regions::parse(
                    "%ttt: off\ntext",
                    DocumentKind::Tex,
                    &Settings {
                        both: markers.clone(),
                        ..Default::default()
                    },
                    Feature::Diagnostics
                )
                .is_empty()
            );
            assert!(serde_json::from_value::<Markers>(serde_json::json!({"off":invalid})).is_err());
        }
        let same = Markers {
            off: "same".into(),
            on: "same".into(),
        };
        assert!(same.validate().unwrap_err().contains("different"));
        assert!(
            Markers {
                off: "x".repeat(128),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            Markers {
                off: "界".repeat(43),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_value::<Settings>(serde_json::json!({"formatting":{"unknown":true}}))
                .is_err()
        );
    }

    fn suppressed(source: &str, kind: DocumentKind) -> Vec<usize> {
        let parsed = parse(source, kind);
        (0..=source.lines().count() + 2)
            .filter(|&line| parsed.contains_line(line))
            .collect()
    }

    #[test]
    fn switches_suppress_only_between_standalone_markers_in_both_languages() {
        for (prefix, kind) in [("%", DocumentKind::Tex), ("//", DocumentKind::Typst)] {
            for newline in ["\n", "\r\n"] {
                let source = [
                    "Visible before".to_owned(),
                    format!("  {prefix}ttt: off  "),
                    "Hidden typo".into(),
                    "Hidden compile error".into(),
                    format!("\t{prefix} ttt: on\t"),
                    "Visible after".into(),
                ]
                .join(newline);
                assert_eq!(suppressed(&source, kind), [3, 4], "{source}");
            }
        }
    }

    #[test]
    fn switches_are_idempotent_support_multiple_regions_and_end_at_eof() {
        let source = "%ttt: on\nvisible\n%ttt: off\nhidden\n%ttt: off\nstill hidden\n%ttt: on\nvisible\n%ttt: off\nhidden at EOF";
        let parsed = parse(source, DocumentKind::Tex);
        assert_eq!(parsed.lines, [4..7, 10..11]);
        assert_eq!(suppressed(source, DocumentKind::Tex), [4, 5, 6, 10]);
        for source in ["%ttt: off", "%ttt: off\n%ttt: on"] {
            assert!(parse(source, DocumentKind::Tex).lines.is_empty());
        }
    }

    #[test]
    fn unclosed_region_includes_the_final_empty_source_line_but_not_beyond_eof() {
        let parsed = parse("%ttt: off\nhidden\n", DocumentKind::Tex);
        assert!(!parsed.contains_line(0));
        assert!(!parsed.contains_line(1));
        assert!(parsed.contains_line(2));
        assert!(parsed.contains_line(3));
        assert!(!parsed.contains_line(4));
        assert!(!parsed.contains_line(usize::MAX));
    }

    #[test]
    fn directives_are_strict_not_prose_trailing_comments_or_unknown_switches() {
        for (prefix, kind) in [("%", DocumentKind::Tex), ("//", DocumentKind::Typst)] {
            for directive in [
                "mention ttt: off",
                "ttt: off followed by explanation",
                "TTT: on",
                "ttt: ON",
                "ttt : nope",
                "ttt: enable",
                "ttt:",
            ] {
                let source = format!("{prefix} {directive}\nvisible");
                assert!(parse(&source, kind).is_empty(), "{source}");
            }
            let source = format!("some code {prefix} ttt: off\nvisible");
            assert!(parse(&source, kind).is_empty());
        }
    }

    #[test]
    fn language_comment_markers_never_apply_to_the_other_language_or_plain_text() {
        for source in ["%ttt: off\nhidden", "//ttt: off\nhidden"] {
            for kind in [DocumentKind::Text, DocumentKind::Pdf, DocumentKind::Image] {
                assert!(parse(source, kind).lines.is_empty());
            }
        }
        assert!(
            parse("%ttt: off\nvisible", DocumentKind::Typst)
                .lines
                .is_empty()
        );
        assert!(
            parse("//ttt: off\nvisible", DocumentKind::Tex)
                .lines
                .is_empty()
        );
    }

    #[test]
    fn typst_strings_raw_fences_and_block_comments_cannot_start_or_stop_regions() {
        for sample in [
            "#let example = \"//ttt: off\"",
            "#let example = \"example\n//ttt: off\nhidden?\"",
            "`//ttt: off`",
            "```typ\n//ttt: off\n```",
            "/*\n//ttt: off\n*/",
            "/* nested /* //ttt: off */ comments */",
            "\\/\\/ttt: off",
        ] {
            let source = format!("{sample}\nvisible");
            assert!(
                parse(&source, DocumentKind::Typst).lines.is_empty(),
                "{source}"
            );
        }
        let source = "//ttt: off\n```typ\n//ttt: on\n```\nstill hidden\n//ttt: on\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Typst), [2, 3, 4, 5]);
    }

    #[test]
    fn tex_escaped_percent_and_inline_literal_examples_are_not_directives() {
        for sample in [
            "\\%ttt: off",
            "\\verb|%ttt: off|",
            "\\lstinline[language=TeX]{\n%ttt: off\n}",
            "\\mintinline{tex}{\n%ttt: off\n}",
            "\\verb§%ttt: off§",
        ] {
            let source = format!("{sample}\nvisible");
            assert!(
                parse(&source, DocumentKind::Tex).lines.is_empty(),
                "{source}"
            );
        }
        // An escaped backslash does not consume the following percent, but the
        // comment is still not standalone and therefore is not a directive.
        assert!(
            parse("\\\\%ttt: off\nvisible", DocumentKind::Tex)
                .lines
                .is_empty()
        );
        let source = "\\verb|unterminated\n%ttt: off\nhidden\n%ttt: on\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [3]);
    }

    #[test]
    fn tex_inline_literal_headers_keep_real_comments_but_exclude_payload_comments() {
        for command in [
            "\\lstinline",
            "\\lstinline[language=TeX]",
            "\\mintinline",
            "\\mintinline[breaklines]",
        ] {
            let payload = if command.starts_with("\\mintinline") {
                "{tex}{\n%ttt: on\n}"
            } else {
                "{\n%ttt: on\n}"
            };
            let source =
                format!("{command}\n%ttt: off\n{payload}\nstill hidden\n%ttt: on\nvisible");
            assert_eq!(
                suppressed(&source, DocumentKind::Tex),
                [3, 4, 5, 6],
                "{source}"
            );
        }
        let source = "%ttt: off\n\\mintinline\n%ttt: on\n{tex}{literal}\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [2]);
        let source = "\\mintinline{tex\n%ttt: off\n}{literal}\nstill hidden\n%ttt: on\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [3, 4]);
    }

    #[test]
    fn tex_verbatim_environments_and_their_unclosed_payloads_do_not_switch() {
        for environment in [
            "verbatim",
            "verbatim*",
            "Verbatim",
            "BVerbatim",
            "LVerbatim",
            "SaveVerbatim",
            "VerbatimOut",
            "lstlisting",
            "minted",
            "comment",
            "filecontents",
            "filecontents*",
        ] {
            for closing in [format!("\\end{{{environment}}}"), String::new()] {
                let source = format!("\\begin{{{environment}}}\n%ttt: off\n{closing}\nvisible");
                assert!(
                    parse(&source, DocumentKind::Tex).lines.is_empty(),
                    "{source}"
                );
            }
        }
        let source = "%ttt: off\n\\begin{verbatim}\n%ttt: on\n\\end{verbatim}\nstill hidden\n%ttt: on\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [2, 3, 4, 5]);
    }

    #[test]
    fn tex_comment_environment_requires_an_exact_standalone_closing_line() {
        for fake_end in [
            "Example: \\end{comment} closes this environment.",
            " \\end{comment}",
            "\\end{comment} trailing text",
            "\\end{comment}% trailing comment",
            "\\\\end{comment}",
        ] {
            for newline in ["\n", "\r\n"] {
                let source = [
                    "\\begin{comment}",
                    fake_end,
                    "%ttt: off",
                    "\\end{comment}",
                    "visible",
                    "%ttt: off",
                    "hidden",
                    "%ttt: on",
                    "visible again",
                ]
                .join(newline);
                assert_eq!(suppressed(&source, DocumentKind::Tex), [7], "{source}");
            }
        }
        let source = "\\begin{comment}\\end{comment}\n%ttt: off\n\\end{comment}\nvisible";
        assert!(parse(source, DocumentKind::Tex).lines.is_empty());
        let source = "\\begin{comment}\nExample: \\end{comment}\n%ttt: off\nno real closing marker";
        assert!(parse(source, DocumentKind::Tex).lines.is_empty());
    }

    #[test]
    fn tex_environment_headers_allow_spacing_and_real_comments() {
        let source = "\\begin\n%ttt: off\n{verbatim}\n%ttt: on\n\\end{verbatim}\nstill hidden\n%ttt: on\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Tex), [3, 4, 5, 6]);
        let source = "\\begin% header\n {verbatim}\n%ttt: off\n\\end{verbatim}\nvisible";
        assert!(parse(source, DocumentKind::Tex).lines.is_empty());
    }

    #[test]
    fn unicode_content_and_comments_preserve_canonical_source_line_numbers() {
        let source =
            "中文😀\n// unrelated é\n//ttt: off\n中文 tyop\n// also hidden\n//ttt: on\nvisible";
        assert_eq!(suppressed(source, DocumentKind::Typst), [4, 5]);
        assert!(
            parse("ordinary text without directives", DocumentKind::Typst)
                .lines
                .is_empty()
        );
    }
}
