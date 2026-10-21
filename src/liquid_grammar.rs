//! Selection-scoped, source-preserving input and edit mapping for manual grammar.
//!
//! This module does no inference or document mutation. Call it on the grammar
//! worker: establishing syntax requires parsing the complete original source.
use crate::document::DocumentKind;
use harper_core::{Token, TokenKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ops::Range;

const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_SELECTED_BYTES: usize = 1024 * 1024;
const MAX_FRAGMENT_SCALARS: usize = MAX_SOURCE_BYTES;
const MAX_DIFF_ATOMS: usize = 1_024;
const MAX_FRAGMENTS: usize = 16_384;

/// How source that cannot be edited as prose is presented to the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProtectedContent {
    #[default]
    Placeholder,
    Split,
}
impl ProtectedContent {
    pub(crate) const ALL: [Self; 2] = [Self::Placeholder, Self::Split];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Placeholder => "Placeholder",
            Self::Split => "Split passages",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FormattedText {
    #[default]
    Include,
    Skip,
}
impl FormattedText {
    pub(crate) const ALL: [Self; 2] = [Self::Include, Self::Skip];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Include => "Include",
            Self::Skip => "Skip",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct InputOptions {
    pub(crate) protected_content: ProtectedContent,
    pub(crate) placeholder: String,
    pub(crate) formatted_text: FormattedText,
}
impl Default for InputOptions {
    fn default() -> Self {
        Self {
            protected_content: ProtectedContent::Placeholder,
            placeholder: "x".into(),
            formatted_text: FormattedText::Include,
        }
    }
}
impl InputOptions {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(1..=24).contains(&self.placeholder.len())
            || !self
                .placeholder
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic())
        {
            return Err(
                "liquid_input.placeholder must contain 1–24 ASCII letters (for example x or math)."
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceEdit {
    /// Unicode scalar coordinates in the unchanged complete source snapshot.
    pub(crate) range: Range<usize>,
    pub(crate) replacement: String,
}

/// A completed model chunk, in UTF-8 byte coordinates of its original prose fragment.
#[derive(Clone, Debug)]
pub(crate) struct ChunkCorrection {
    pub(crate) fragment: usize,
    pub(crate) range: Range<usize>,
    pub(crate) corrected: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Fragment {
    pub(crate) text: String,
    origins: Vec<Origin>,
    discontinuous_words: Vec<Range<usize>>,
    source_range: Range<usize>,
    fenced_start: bool,
    fenced_end: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Selection {
    pub(crate) fragments: Vec<Fragment>,
    range: Range<usize>,
    source_hash: [u8; 32],
    source_chars: Vec<char>,
    fragment_bytes: Vec<Vec<usize>>,
    kind: DocumentKind,
    options: InputOptions,
    tex_rules: Option<std::sync::Arc<crate::writing::prose_rules::TexProseRules>>,
}

#[derive(Clone, Debug)]
enum Origin {
    Literal(usize),
    Whitespace(Range<usize>),
    Separator,
    Opaque(Range<usize>),
}

impl Origin {
    fn range(&self) -> Option<Range<usize>> {
        match self {
            Self::Literal(at) => Some(*at..at + 1),
            Self::Whitespace(range) | Self::Opaque(range) => Some(range.clone()),
            Self::Separator => None,
        }
    }
}

impl Selection {
    #[cfg(test)]
    pub(crate) fn new(
        source: &str,
        kind: DocumentKind,
        range: Range<usize>,
    ) -> Result<Self, String> {
        Self::with_options(source, kind, range, InputOptions::default())
    }

    #[cfg(test)]
    pub(crate) fn with_options(
        source: &str,
        kind: DocumentKind,
        range: Range<usize>,
        options: InputOptions,
    ) -> Result<Self, String> {
        Self::with_prose_rules(source, kind, range, options, None)
    }

    pub(crate) fn with_prose_rules(
        source: &str,
        kind: DocumentKind,
        range: Range<usize>,
        options: InputOptions,
        rules: Option<&crate::writing::prose_rules::TexProseRules>,
    ) -> Result<Self, String> {
        if let Some(rules) = rules {
            rules.validate()?;
        }
        options.validate()?;
        if !matches!(kind, DocumentKind::Typst | DocumentKind::Tex) {
            return Err("Manual grammar checking supports Typst and TeX documents.".into());
        }
        if source.len() > MAX_SOURCE_BYTES {
            return Err(
                "This document is too large for manual grammar checking (2 MiB limit).".into(),
            );
        }
        if range.start >= range.end {
            return Err("Select some text to check its grammar.".into());
        }
        let chars: Vec<_> = source.chars().collect();
        if range.end > chars.len() {
            return Err("The selected text is no longer available.".into());
        }
        if chars[range.clone()]
            .iter()
            .map(|ch| ch.len_utf8())
            .sum::<usize>()
            > MAX_SELECTED_BYTES
        {
            return Err("Select at most 1 MiB of text for one grammar check.".into());
        }
        let tokens = crate::writing::liquid_prose_tokens(
            &chars,
            kind,
            options.formatted_text == FormattedText::Skip,
            rules,
        );
        let mut fragments = Vec::new();
        let mut builder = Builder::new(range.start);
        for token in tokens {
            if token.span.start > token.span.end || token.span.end > chars.len() {
                return Err("The selected source could not be mapped safely.".into());
            }
            if token.kind.is_paragraph_break() {
                if token.span.start <= range.end && token.span.end >= range.start {
                    let edge = token.span.start.clamp(builder.start, range.end);
                    // A parser may emit an empty paragraph edge at the end of
                    // a skipped wrapper. It must not erase the excluded edge.
                    let fenced_start = builder.fenced_start && !builder.has_word;
                    builder.finish(edge, &mut fragments)?;
                    builder = Builder::new(token.span.end.clamp(edge, range.end));
                    builder.fenced_start = fenced_start;
                }
                continue;
            }
            let clipped = token.span.start.max(range.start)..token.span.end.min(range.end);
            if clipped.is_empty() {
                continue;
            }
            if matches!(token.kind, TokenKind::HeadingStart) {
                continue;
            }
            if hidden(&token)
                || ((token.kind.is_word() || token.kind.is_number())
                    && (clipped.start != token.span.start || clipped.end != token.span.end))
            {
                match options.protected_content {
                    ProtectedContent::Placeholder => builder.opaque(clipped, &options.placeholder),
                    ProtectedContent::Split => {
                        // Do not turn omitted mathematics into adjacent prose, or
                        // permit insertions to choose either side of that gap.
                        let end = clipped.start.max(builder.start);
                        builder.fenced_end = true;
                        builder.finish(end, &mut fragments)?;
                        builder = Builder::new(clipped.end);
                        builder.fenced_start = true;
                    }
                }
            } else if token.kind.is_whitespace() {
                let origin = chars[clipped.clone()]
                    .iter()
                    .all(|ch| ch.is_whitespace())
                    .then_some(clipped);
                builder.whitespace(origin);
            } else {
                for at in clipped {
                    if chars[at].is_whitespace() {
                        builder.whitespace(Some(at..at + 1));
                    } else {
                        builder.literal(chars[at], at)?;
                        builder.has_word |= token.kind.is_word();
                    }
                }
            }
        }
        builder.finish(range.end, &mut fragments)?;
        if fragments.is_empty() {
            return Err("The selection contains no complete prose words to check.".into());
        }
        let fragment_bytes = fragments
            .iter()
            .map(|fragment| {
                fragment
                    .text
                    .char_indices()
                    .map(|(byte, _)| byte)
                    .chain(std::iter::once(fragment.text.len()))
                    .collect()
            })
            .collect();
        Ok(Self {
            fragments,
            fragment_bytes,
            range,
            source_hash: Sha256::digest(source.as_bytes()).into(),
            source_chars: chars,
            kind,
            options,
            tex_rules: rules.cloned().map(std::sync::Arc::new),
        })
    }

    /// Returns an atomic proposal. Any unsafe edit rejects the entire response.
    /// Call again immediately before applying a reviewed proposal to reject a
    /// result whose document changed while inference or review was in progress.
    #[cfg(test)]
    pub(crate) fn map_corrections(
        &self,
        current_source: &str,
        corrected: &[String],
    ) -> Result<Vec<SourceEdit>, String> {
        if corrected.len() != self.fragments.len() {
            return Err("The grammar response did not match the selected fragments.".into());
        }
        let chunks: Vec<_> = self
            .fragments
            .iter()
            .zip(corrected)
            .enumerate()
            .map(|(fragment, (original, corrected))| ChunkCorrection {
                fragment,
                range: 0..original.text.len(),
                corrected: corrected.clone(),
            })
            .collect();
        self.map_chunk_corrections(current_source, &chunks)
    }

    /// Map one completed chunk immediately without reparsing the whole document.
    /// These incremental proposals remain unapplyable until the final complete
    /// response passes `map_chunk_corrections` and its syntax reparse.
    pub(crate) fn map_chunk(&self, chunk: &ChunkCorrection) -> Result<Vec<SourceEdit>, String> {
        let fragment = self
            .fragments
            .get(chunk.fragment)
            .ok_or("The grammar response contains an unknown fragment.")?;
        let input = fragment
            .text
            .get(chunk.range.clone())
            .ok_or("The grammar response contains an invalid chunk range.")?;
        if chunk.corrected.chars().count() > MAX_FRAGMENT_SCALARS {
            return Err("The grammar response is too long to map safely.".into());
        }
        let offset = self.fragment_bytes[chunk.fragment]
            .binary_search(&chunk.range.start)
            .map_err(|_| "The grammar response contains an invalid chunk boundary.")?;
        let mut edits = Vec::new();
        for change in diff(input, &chunk.corrected)? {
            if unsafe_replacement(self.kind, &change.replacement) {
                return Err(
                    "A correction introduces document syntax. Edit that passage manually.".into(),
                );
            }
            let range = fragment.map_edit(
                &self.source_chars,
                change.range.start + offset..change.range.end + offset,
            )?;
            if range.start < self.range.start || range.end > self.range.end {
                return Err("A correction would change text outside the selection.".into());
            }
            let removed = self
                .source_chars
                .get(range.clone())
                .ok_or("A correction has invalid source coordinates.")?;
            if removed.iter().any(|&ch| reserved_syntax(self.kind, ch)) {
                return Err(
                    "A correction would remove document syntax. Edit that passage manually.".into(),
                );
            }
            edits.push(SourceEdit {
                range,
                replacement: change.replacement,
            });
        }
        Ok(edits)
    }

    /// Validate all completed chunks together before Apply can be enabled.
    /// Unprocessed portions remain byte-for-byte unchanged in partial reviews.
    pub(crate) fn map_chunk_corrections(
        &self,
        current_source: &str,
        chunks: &[ChunkCorrection],
    ) -> Result<Vec<SourceEdit>, String> {
        let hash: [u8; 32] = Sha256::digest(current_source.as_bytes()).into();
        if hash != self.source_hash {
            return Err(
                "The document changed. Check the selection again before applying corrections."
                    .into(),
            );
        }
        let mut ordered: Vec<_> = chunks.iter().collect();
        ordered.sort_unstable_by_key(|chunk| (chunk.fragment, chunk.range.start));
        if ordered.windows(2).any(|pair| {
            pair[0].fragment == pair[1].fragment
                && (pair[0].range.end > pair[1].range.start
                    || pair[0].range.start == pair[1].range.start)
        }) {
            return Err("The grammar chunks overlap and cannot be applied safely.".into());
        }
        let mut corrected: Vec<String> = self.fragments.iter().map(|_| String::new()).collect();
        let mut cursors = vec![0; self.fragments.len()];
        let mut edits = Vec::new();
        for chunk in ordered {
            edits.extend(self.map_chunk(chunk)?);
            corrected[chunk.fragment].push_str(
                &self.fragments[chunk.fragment].text[cursors[chunk.fragment]..chunk.range.start],
            );
            corrected[chunk.fragment].push_str(&chunk.corrected);
            cursors[chunk.fragment] = chunk.range.end;
        }
        for (fragment, (output, cursor)) in
            self.fragments.iter().zip(corrected.iter_mut().zip(cursors))
        {
            output.push_str(&fragment.text[cursor..]);
        }
        if corrected
            .iter()
            .any(|text| text.chars().count() > MAX_FRAGMENT_SCALARS)
        {
            return Err("The grammar response is too long to map safely.".into());
        }
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        if edits.windows(2).any(|pair| {
            pair[0].range.end > pair[1].range.start || pair[0].range.start == pair[1].range.start
        }) {
            return Err("The corrections overlap and cannot be applied safely.".into());
        }
        if edits.is_empty() {
            return Ok(edits);
        }
        // A coordinate-safe edit can still turn prose into Typst/TeX syntax.
        // Reparse only the in-memory proposal and require identical admission.
        let mut candidate = String::with_capacity(current_source.len());
        let mut cursor = 0;
        for edit in &edits {
            candidate.extend(self.source_chars[cursor..edit.range.start].iter());
            candidate.push_str(&edit.replacement);
            cursor = edit.range.end;
        }
        candidate.extend(self.source_chars[cursor..].iter());
        let new_end = self.range.end as isize
            + edits
                .iter()
                .map(|edit| {
                    edit.replacement.chars().count() as isize
                        - (edit.range.end - edit.range.start) as isize
                })
                .sum::<isize>();
        let projected = Self::with_prose_rules(
            &candidate,
            self.kind,
            self.range.start..new_end as usize,
            self.options.clone(),
            self.tex_rules.as_deref(),
        )
        .map_err(|_| {
            "A correction changes how the document is parsed. Edit that passage manually."
                .to_owned()
        })?;
        if projected
            .fragments
            .iter()
            .map(|fragment| &fragment.text)
            .ne(corrected.iter())
        {
            return Err(
                "A correction changes document syntax or a protected fragment boundary.".into(),
            );
        }
        Ok(edits)
    }
}

fn hidden(token: &Token) -> bool {
    matches!(
        token.kind,
        TokenKind::Unlintable
            | TokenKind::Regexish
            | TokenKind::Url
            | TokenKind::EmailAddress
            | TokenKind::Hostname
    )
}

struct Builder {
    start: usize,
    chars: Vec<char>,
    origins: Vec<Origin>,
    has_word: bool,
    fenced_start: bool,
    fenced_end: bool,
}

impl Builder {
    fn new(start: usize) -> Self {
        Self {
            start,
            chars: Vec::new(),
            origins: Vec::new(),
            has_word: false,
            fenced_start: false,
            fenced_end: false,
        }
    }

    fn push(&mut self, ch: char, origin: Origin) {
        self.chars.push(ch);
        self.origins.push(origin);
    }

    fn whitespace(&mut self, range: Option<Range<usize>>) {
        if self.chars.is_empty() {
            return;
        }
        if self.chars.last() == Some(&' ') {
            let last = self.origins.last_mut().unwrap();
            if let (Origin::Whitespace(previous), Some(next)) = (&mut *last, range)
                && previous.end == next.start
            {
                previous.end = next.end;
                return;
            }
            *last = Origin::Separator;
        } else {
            self.push(' ', range.map_or(Origin::Separator, Origin::Whitespace));
        }
    }

    fn opaque(&mut self, range: Range<usize>, placeholder: &str) {
        if matches!(self.origins.last(), Some(Origin::Opaque(_))) {
            for origin in self.origins.iter_mut().rev() {
                let Origin::Opaque(previous) = origin else {
                    break;
                };
                previous.end = previous.end.max(range.end);
            }
            return;
        }
        self.whitespace(None);
        for ch in placeholder.chars() {
            self.push(ch, Origin::Opaque(range.clone()));
        }
    }

    fn literal(&mut self, ch: char, at: usize) -> Result<(), String> {
        if self
            .origins
            .iter()
            .rev()
            .find_map(Origin::range)
            .is_some_and(|last| last.end > at)
        {
            return Err(
                "The selected source has overlapping prose ranges and cannot be mapped safely."
                    .into(),
            );
        }
        if matches!(self.origins.last(), Some(Origin::Opaque(_))) && ch.is_alphanumeric() {
            self.whitespace(None);
        }
        self.push(ch, Origin::Literal(at));
        Ok(())
    }

    fn finish(mut self, end: usize, fragments: &mut Vec<Fragment>) -> Result<(), String> {
        while self.chars.last() == Some(&' ') {
            self.chars.pop();
            self.origins.pop();
        }
        if self.has_word {
            if self.chars.len() > MAX_FRAGMENT_SCALARS {
                return Err("A selected paragraph is too long. Select a shorter passage.".into());
            }
            if fragments.len() >= MAX_FRAGMENTS {
                return Err("Select fewer paragraphs for one grammar check.".into());
            }
            let discontinuous_words = discontinuous_words(&self.chars, &self.origins);
            fragments.push(Fragment {
                text: self.chars.into_iter().collect(),
                discontinuous_words,
                origins: self.origins,
                source_range: self.start..end,
                fenced_start: self.fenced_start,
                fenced_end: self.fenced_end,
            });
        }
        Ok(())
    }
}

/// Cache only words whose source positions cross omitted markup. Most prose
/// has no such spans; queries are logarithmic even in markup-heavy paragraphs.
fn discontinuous_words(chars: &[char], origins: &[Origin]) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if !word_char(chars[start]) {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < chars.len() && word_char(chars[end]) {
            end += 1;
        }
        if origins[start..end]
            .windows(2)
            .any(|pair| match (pair[0].range(), pair[1].range()) {
                (Some(left), Some(right)) => left.end != right.start,
                _ => true,
            })
        {
            result.push(start..end);
        }
        start = end;
    }
    result
}

#[cfg(test)]
std::thread_local! {
    static WORD_FENCE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Fragment {
    fn map_edit(&self, source: &[char], range: Range<usize>) -> Result<Range<usize>, String> {
        if range.start > range.end || range.end > self.origins.len() {
            return Err("A correction has invalid fragment coordinates.".into());
        }
        // The expensive lexical/source check was cached once during projection.
        // Only the first discontinuous word ending after this edit can overlap.
        let candidate = self.discontinuous_words.partition_point(|word| {
            #[cfg(test)]
            WORD_FENCE_PROBES.with(|probes| probes.set(probes.get() + 1));
            word.end <= range.start
        });
        if self.discontinuous_words.get(candidate).is_some_and(|word| {
            #[cfg(test)]
            WORD_FENCE_PROBES.with(|probes| probes.set(probes.get() + 1));
            range.end > word.start
        }) {
            return Err("A correction touches a word split across formatting syntax.".into());
        }
        if range.is_empty() {
            return self.insertion(source, range.start).map(|at| at..at);
        }
        let mut mapped: Option<Range<usize>> = None;
        for origin in &self.origins[range] {
            if matches!(origin, Origin::Opaque(_)) {
                return Err("A correction changes protected mathematics, code or a link.".into());
            }
            let next = origin
                .range()
                .ok_or_else(|| "A correction crosses omitted document syntax.".to_owned())?;
            if let Some(previous) = &mut mapped {
                if previous.end != next.start {
                    return Err("A correction crosses omitted document syntax.".into());
                }
                previous.end = next.end;
            } else {
                mapped = Some(next);
            }
        }
        mapped.ok_or_else(|| "The correction has no editable source text.".into())
    }

    fn insertion(&self, source: &[char], index: usize) -> Result<usize, String> {
        if (index == 0 && self.fenced_start) || (index == self.origins.len() && self.fenced_end) {
            return Err(
                "An insertion next to excluded content has an ambiguous source position.".into(),
            );
        }
        for direction in [-1isize, 1] {
            let mut cursor = if direction < 0 {
                index as isize - 1
            } else {
                index as isize
            };
            while cursor >= 0
                && (cursor as usize) < self.origins.len()
                && matches!(
                    self.origins[cursor as usize],
                    Origin::Whitespace(_) | Origin::Separator
                )
            {
                cursor += direction;
            }
            if cursor >= 0
                && (cursor as usize) < self.origins.len()
                && matches!(self.origins[cursor as usize], Origin::Opaque(_))
            {
                return Err(
                    "An insertion next to mathematics or code has an ambiguous source position."
                        .into(),
                );
            }
        }
        if index == 0 {
            let start = self
                .origins
                .first()
                .and_then(Origin::range)
                .map(|range| range.start)
                .ok_or_else(|| "The fragment start has no unique source position.".to_owned())?;
            if source
                .get(self.source_range.start..start)
                .is_some_and(|gap| gap.iter().all(|ch| ch.is_whitespace()))
            {
                return Ok(start);
            }
        } else if index == self.origins.len() {
            let end = self
                .origins
                .last()
                .and_then(Origin::range)
                .map(|range| range.end)
                .ok_or_else(|| "The fragment end has no unique source position.".to_owned())?;
            if source
                .get(end..self.source_range.end)
                .is_some_and(|gap| gap.iter().all(|ch| ch.is_whitespace()))
            {
                return Ok(end);
            }
        } else if let (Some(left), Some(right)) =
            (self.origins[index - 1].range(), self.origins[index].range())
            && left.end == right.start
        {
            return Ok(right.start);
        }
        Err("The insertion crosses formatting or a protected fragment boundary.".into())
    }
}

fn unsafe_replacement(kind: DocumentKind, replacement: &str) -> bool {
    replacement.chars().any(|ch| reserved_syntax(kind, ch))
}

fn reserved_syntax(kind: DocumentKind, ch: char) -> bool {
    ch.is_control()
        || match kind {
            DocumentKind::Tex => "\\{}$%&#_^~".contains(ch),
            DocumentKind::Typst => "\\{}$#[]<>*_`@/\"=".contains(ch),
            _ => true,
        }
}

#[derive(Clone, Copy)]
struct Atom {
    start: usize,
    end: usize,
}

fn word_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '\'' | '’')
}

fn atoms(chars: &[char]) -> Result<Vec<Atom>, String> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start + 1;
        if word_char(chars[start]) {
            while end < chars.len() && word_char(chars[end]) {
                end += 1;
            }
        } else if chars[start].is_whitespace() {
            while end < chars.len() && chars[end].is_whitespace() {
                end += 1;
            }
        }
        result.push(Atom { start, end });
        if result.len() > MAX_DIFF_ATOMS {
            return Err(
                "The passage has too many tokens to map safely. Select a shorter passage.".into(),
            );
        }
        start = end;
    }
    Ok(result)
}

/// Bounded word-level LCS keeps edits from masquerading as local character
/// changes inside a word that crosses omitted command syntax.
fn diff(original: &str, corrected: &str) -> Result<Vec<SourceEdit>, String> {
    if original == corrected {
        return Ok(Vec::new());
    }
    let left: Vec<_> = original.chars().collect();
    let right: Vec<_> = corrected.chars().collect();
    let a = atoms(&left)?;
    let b = atoms(&right)?;
    let width = b.len() + 1;
    let mut table = vec![0u16; (a.len() + 1) * width];
    let equal = |i: usize, j: usize| left[a[i].start..a[i].end] == right[b[j].start..b[j].end];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i * width + j] = if equal(i, j) {
                1 + table[(i + 1) * width + j + 1]
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut edits = Vec::new();
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && equal(i, j) {
            i += 1;
            j += 1;
            continue;
        }
        let (start_i, start_j) = (i, j);
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && equal(i, j) {
                break;
            }
            if j < b.len()
                && (i == a.len() || table[i * width + j + 1] > table[(i + 1) * width + j])
            {
                j += 1;
            } else {
                i += 1;
            }
        }
        let start = a.get(start_i).map_or(left.len(), |atom| atom.start);
        let end = a.get(i).map_or(left.len(), |atom| atom.start);
        let new_start = b.get(start_j).map_or(right.len(), |atom| atom.start);
        let new_end = b.get(j).map_or(right.len(), |atom| atom.start);
        edits.push(SourceEdit {
            range: start..end,
            replacement: right[new_start..new_end].iter().collect(),
        });
    }
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str, edits: &[SourceEdit]) -> String {
        let mut chars: Vec<_> = source.chars().collect();
        for edit in edits.iter().rev() {
            chars.splice(edit.range.clone(), edit.replacement.chars());
        }
        chars.into_iter().collect()
    }

    fn selected(source: &str, text: &str) -> Range<usize> {
        let byte = source.find(text).unwrap();
        let start = source[..byte].chars().count();
        start..start + text.chars().count()
    }

    #[test]
    fn unicode_selection_preserves_math_markup_and_everything_outside_selection() {
        for (kind, source) in [
            (
                DocumentKind::Typst,
                "Outside.\n\nTao’s result $alpha$ give a estimate.\n\nUntouched.",
            ),
            (
                DocumentKind::Tex,
                "Outside.\n\nTao’s result $\\alpha$ give a estimate.\n\nUntouched.",
            ),
        ] {
            let range = selected(
                source,
                &source[source.find("Tao").unwrap()..source.find("\n\nUntouched").unwrap()],
            );
            let selection = Selection::new(source, kind, range.clone()).unwrap();
            assert_eq!(selection.range, range);
            assert_eq!(selection.fragments.len(), 1);
            assert_eq!(
                selection.fragments[0].text,
                "Tao’s result x give a estimate."
            );
            let edits = selection
                .map_corrections(source, &["Tao’s result x gives an estimate.".into()])
                .unwrap();
            assert!(
                edits
                    .iter()
                    .all(|edit| range.start <= edit.range.start && edit.range.end <= range.end)
            );
            assert_eq!(
                apply(source, &edits),
                source.replace("give a estimate", "gives an estimate")
            );
        }
    }

    #[test]
    fn formatting_word_repair_preserves_its_command_and_source_offsets() {
        for (kind, source) in [
            (
                DocumentKind::Typst,
                "Before. The #emph[estmates] hold. After.",
            ),
            (
                DocumentKind::Tex,
                r"Before. The \emph{estmates} hold. After.",
            ),
        ] {
            let selection = Selection::new(source, kind, selected(source, "estmates")).unwrap();
            assert_eq!(selection.fragments[0].text, "estmates");
            let edits = selection
                .map_corrections(source, &["estimates".into()])
                .unwrap();
            assert_eq!(
                edits,
                [SourceEdit {
                    range: selected(source, "estmates"),
                    replacement: "estimates".into()
                }]
            );
            assert_eq!(
                apply(source, &edits),
                source.replace("estmates", "estimates")
            );
        }
    }

    #[test]
    fn empty_partial_word_and_opaque_only_selections_are_not_sent() {
        assert!(Selection::new("A sentence.", DocumentKind::Typst, 0..0).is_err());
        assert!(Selection::new("A sentence.", DocumentKind::Tex, 2..99).is_err());
        let source = "The nonautonmous equation is stable.";
        assert!(
            Selection::new(source, DocumentKind::Typst, selected(source, "autonmous")).is_err()
        );
        for (kind, source) in [
            (DocumentKind::Typst, "$ alpha + beta $"),
            (DocumentKind::Tex, r"\[\alpha + \beta\]"),
        ] {
            assert!(Selection::new(source, kind, 0..source.chars().count()).is_err());
        }
    }

    #[test]
    fn stale_snapshot_and_response_count_are_rejected() {
        let source = "A estimate holds.";
        let selection =
            Selection::new(source, DocumentKind::Typst, 0..source.chars().count()).unwrap();
        assert!(
            selection
                .map_corrections("A estimate holds. ", &["An estimate holds.".into()])
                .unwrap_err()
                .contains("changed")
        );
        assert!(selection.map_corrections(source, &[]).is_err());
    }

    #[test]
    fn split_formatting_words_and_cross_wrapper_deletions_are_rejected() {
        for (kind, source) in [
            (
                DocumentKind::Typst,
                "The non#emph[autonmous] equation is stable.",
            ),
            (
                DocumentKind::Tex,
                r"The non\emph{autonmous} equation is stable.",
            ),
        ] {
            let selection = Selection::new(source, kind, 0..source.chars().count()).unwrap();
            assert_eq!(
                selection.fragments[0].text,
                "The nonautonmous equation is stable."
            );
            assert!(
                selection
                    .map_corrections(source, &["The nonautonomous equation is stable.".into()])
                    .is_err()
            );
        }
    }

    #[test]
    fn protected_objects_and_ambiguous_adjacent_insertions_are_rejected() {
        let source = "The norm $u$ is finite.";
        let selection =
            Selection::new(source, DocumentKind::Typst, 0..source.chars().count()).unwrap();
        for output in ["The norm y is finite.", "The norm of x is finite."] {
            assert!(
                selection.map_corrections(source, &[output.into()]).is_err(),
                "{output}"
            );
        }
        let source = "#emph[Solution exists.]";
        let selection =
            Selection::new(source, DocumentKind::Typst, 0..source.chars().count()).unwrap();
        assert!(
            selection
                .map_corrections(source, &["A Solution exists.".into()])
                .is_err()
        );
        // Changing capitalization makes this a contiguous whole-word
        // replacement, not an ambiguous insertion outside/inside the wrapper.
        let edits = selection
            .map_corrections(source, &["A solution exists.".into()])
            .unwrap();
        assert_eq!(apply(source, &edits), "#emph[A solution exists.]");
        assert_eq!(edits[0].range, selected(source, "Solution"));
    }

    #[test]
    fn syntax_injection_rejects_the_entire_multi_paragraph_proposal() {
        let source = "A estimate holds.\n\nThe result follows.";
        let selection =
            Selection::new(source, DocumentKind::Typst, 0..source.chars().count()).unwrap();
        assert_eq!(selection.fragments.len(), 2);
        assert!(
            selection
                .map_corrections(
                    source,
                    &["An estimate holds.".into(), "#The result follows.".into()]
                )
                .is_err()
        );
        let source = "A estimate holds.";
        let selection =
            Selection::new(source, DocumentKind::Tex, 0..source.chars().count()).unwrap();
        assert!(
            selection
                .map_corrections(source, &[r"An \estimate holds.".into()])
                .is_err()
        );
    }

    #[test]
    fn complete_words_from_a_partial_paragraph_can_be_corrected_locally() {
        let source = "Prefix sentence. This estimate are stable. Suffix sentence.";
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let range = selected(source, "estimate are stable.");
            let selection = Selection::new(source, kind, range.clone()).unwrap();
            assert_eq!(selection.fragments[0].text, "estimate are stable.");
            let edits = selection
                .map_corrections(source, &["estimate is stable.".into()])
                .unwrap();
            assert!(
                edits
                    .iter()
                    .all(|edit| { range.start <= edit.range.start && edit.range.end <= range.end })
            );
            assert_eq!(
                apply(source, &edits),
                source.replace("are stable", "is stable")
            );
        }
    }

    #[test]
    fn admitted_source_syntax_cannot_be_removed_by_a_model() {
        // Exercise the guard even for a parser that admitted a syntax-like
        // punctuation character as literal prose. Coordinate safety alone
        // must never permit deleting a TeX alignment marker.
        let source = "The bound & holds.";
        let selection = Selection {
            fragments: vec![Fragment {
                text: source.into(),
                origins: (0..source.chars().count()).map(Origin::Literal).collect(),
                discontinuous_words: Vec::new(),
                source_range: 0..source.chars().count(),
                fenced_start: false,
                fenced_end: false,
            }],
            range: 0..source.chars().count(),
            source_hash: Sha256::digest(source.as_bytes()).into(),
            source_chars: source.chars().collect(),
            fragment_bytes: vec![
                source
                    .char_indices()
                    .map(|(byte, _)| byte)
                    .chain(std::iter::once(source.len()))
                    .collect(),
            ],
            kind: DocumentKind::Tex,
            options: InputOptions::default(),
            tex_rules: None,
        };
        let error = selection
            .map_corrections(source, &["The bound holds.".into()])
            .unwrap_err();
        assert!(error.contains("remove document syntax"), "{error}");
    }

    #[test]
    fn overlapping_structural_edges_reject_ambiguous_insertions_without_panicking() {
        // Harper's TeX structural break tokens may overlap a previously
        // admitted title. An impossible edge is a refusal, never a slice panic.
        let source: Vec<_> = "A result.".chars().collect();
        let mut fragment = Fragment {
            text: source.iter().collect(),
            origins: (0..source.len()).map(Origin::Literal).collect(),
            discontinuous_words: Vec::new(),
            source_range: 3..source.len(),
            fenced_start: false,
            fenced_end: false,
        };
        assert!(fragment.insertion(&source, 0).is_err());
        fragment.source_range = 0..3;
        assert!(fragment.insertion(&source, source.len()).is_err());

        for text in [
            "This is is a test.\n\n\\section{The mathematical result}",
            "\\documentclass{article}\n\\begin{document}\nA estimate holds.\n\n\\section{Next}\nThe result follows.\n\\end{document}",
        ] {
            let selection =
                Selection::new(text, DocumentKind::Tex, 0..text.chars().count()).unwrap();
            let outputs: Vec<_> = selection
                .fragments
                .iter()
                .map(|fragment| format!("Indeed, {}", fragment.text))
                .collect();
            if let Ok(edits) = selection.map_corrections(text, &outputs) {
                assert!(edits.iter().all(|edit| edit.range.start <= edit.range.end
                    && edit.range.end <= text.chars().count()));
            }
        }
    }

    #[test]
    fn work_limits_fail_explicitly_and_unchanged_responses_produce_no_edits() {
        let source = "a".repeat(MAX_SELECTED_BYTES + 1);
        assert!(
            Selection::new(&source, DocumentKind::Typst, 0..source.len())
                .unwrap_err()
                .contains("1 MiB")
        );
        let source = "The estimate holds.";
        let selection =
            Selection::new(source, DocumentKind::Typst, 0..source.chars().count()).unwrap();
        assert!(
            selection
                .map_corrections(source, &[source.into()])
                .unwrap()
                .is_empty()
        );
        assert!(diff(&"a ".repeat(MAX_DIFF_ATOMS), "b").is_err());
    }

    fn options_selection(source: &str, kind: DocumentKind, options: InputOptions) -> Selection {
        Selection::with_options(source, kind, 0..source.chars().count(), options).unwrap()
    }

    fn texts(selection: &Selection) -> Vec<&str> {
        selection
            .fragments
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect()
    }

    #[test]
    fn input_options_have_explicit_defaults_and_reject_unsafe_placeholders() {
        let options = InputOptions::default();
        assert_eq!(options.protected_content, ProtectedContent::Placeholder);
        assert_eq!(options.formatted_text, FormattedText::Include);
        assert_eq!(options.placeholder, "x");
        assert_eq!(serde_json::from_str::<InputOptions>("{}").unwrap(), options);
        for placeholder in [
            "",
            "a b",
            "x$",
            "math_",
            "é",
            "x\n",
            "123",
            "abcdefghijklmnopqrstuvwxyz",
        ] {
            let options = InputOptions {
                placeholder: placeholder.into(),
                ..Default::default()
            };
            assert!(
                options
                    .validate()
                    .unwrap_err()
                    .contains("liquid_input.placeholder")
            );
            assert!(
                Selection::with_options("A sentence.", DocumentKind::Typst, 0..11, options)
                    .is_err()
            );
        }
        for placeholder in ["x", "math", "ABCDEFGHIJKLMNOPQRSTUVWX"] {
            assert!(
                InputOptions {
                    placeholder: placeholder.into(),
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        assert!(serde_json::from_str::<InputOptions>(r#"{"protected_content":"remove"}"#).is_err());
        assert!(serde_json::from_str::<InputOptions>(r#"{"formatted_text":"invent"}"#).is_err());
    }

    #[test]
    fn custom_placeholders_remain_protected_and_reprojection_keeps_the_policy() {
        for (kind, source) in [
            (DocumentKind::Typst, "Tao’s estmates $alpha$ are continous."),
            (DocumentKind::Tex, r"Tao’s estmates $\alpha$ are continous."),
        ] {
            let selection = options_selection(
                source,
                kind,
                InputOptions {
                    placeholder: "math".into(),
                    ..Default::default()
                },
            );
            assert_eq!(texts(&selection), ["Tao’s estmates math are continous."]);
            let edits = selection
                .map_corrections(source, &["Tao’s estimates math are continuous.".into()])
                .unwrap();
            assert_eq!(
                apply(source, &edits),
                source
                    .replace("estmates", "estimates")
                    .replace("continous", "continuous")
            );
            assert!(
                selection
                    .map_corrections(source, &["Tao’s estimates norm are continuous.".into()])
                    .is_err()
            );
            assert!(
                selection
                    .map_corrections(source, &["Tao’s estimates are continuous.".into()])
                    .is_err()
            );
        }
    }

    #[test]
    fn split_policy_keeps_disconnected_fragments_and_fences_their_insertions() {
        for (kind, source) in [
            (
                DocumentKind::Typst,
                "é Estmates hold $alpha$ continous bounds remain.",
            ),
            (
                DocumentKind::Tex,
                r"é Estmates hold $\alpha$ continous bounds remain.",
            ),
        ] {
            let selection = options_selection(
                source,
                kind,
                InputOptions {
                    protected_content: ProtectedContent::Split,
                    ..Default::default()
                },
            );
            assert_eq!(
                texts(&selection),
                ["é Estmates hold", "continous bounds remain."]
            );
            let edits = selection
                .map_corrections(
                    source,
                    &[
                        "é Estimates hold".into(),
                        "continuous bounds remain.".into(),
                    ],
                )
                .unwrap();
            assert_eq!(
                apply(source, &edits),
                source
                    .replace("Estmates", "Estimates")
                    .replace("continous", "continuous")
            );
            assert!(
                selection
                    .map_corrections(
                        source,
                        &["é Estmates hold.".into(), "continous bounds remain.".into()]
                    )
                    .is_err()
            );
            assert!(
                selection
                    .map_corrections(
                        source,
                        &[
                            "é Estmates hold".into(),
                            "Now continous bounds remain.".into()
                        ]
                    )
                    .is_err()
            );
            assert!(
                selection
                    .map_corrections(
                        source,
                        &["é Estimates hold continuous bounds remain.".into()]
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn formatting_skip_hides_nested_bodies_without_changing_automatic_prose() {
        for (kind, formats) in [
            (
                DocumentKind::Typst,
                vec![
                    "*secret typo*",
                    "_secret typo_",
                    "#emph[secret #strong[typo]]",
                    "#strong[secret typo]",
                    "#text(fill: red)[secret typo]",
                    "#emph(body: [secret typo])",
                    "#std.emph[secret typo]",
                ],
            ),
            (
                DocumentKind::Tex,
                vec![
                    r"\emph{secret \textbf{typo}}",
                    r"\textbf{secret typo}",
                    r"\textit{secret typo}",
                    r"\text{secret typo}",
                    r"\underline{secret typo}",
                    r"\textrm{secret typo}",
                ],
            ),
        ] {
            for formatted in formats {
                let source = format!("é The estmates {formatted} are continous.");
                let chars: Vec<_> = source.chars().collect();
                let baseline = crate::writing::liquid_prose_tokens(&chars, kind, false, None);
                let selection = options_selection(
                    &source,
                    kind,
                    InputOptions {
                        formatted_text: FormattedText::Skip,
                        ..Default::default()
                    },
                );
                assert_eq!(
                    texts(&selection),
                    ["é The estmates x are continous."],
                    "{source}"
                );
                let edits = selection
                    .map_corrections(&source, &["é The estimates x are continuous.".into()])
                    .unwrap();
                assert_eq!(
                    apply(&source, &edits),
                    source
                        .replace("estmates", "estimates")
                        .replace("continous", "continuous")
                );
                assert!(
                    selection
                        .map_corrections(&source, &["é The estmates secret are continous.".into()])
                        .is_err()
                );
                // Skip is confined to manual admission, with no global setting
                // or cache mutation changing automatic Harper's next parse.
                let after = crate::writing::liquid_prose_tokens(&chars, kind, false, None);
                assert_eq!(baseline, after);
                let range = selected(&source, "secret");
                assert!(
                    Selection::with_options(
                        &source,
                        kind,
                        range,
                        InputOptions {
                            formatted_text: FormattedText::Skip,
                            ..Default::default()
                        }
                    )
                    .is_err()
                );
            }
        }
    }

    #[test]
    fn skipped_formatting_can_split_without_editing_wrappers_or_inserting_at_the_gap() {
        for (kind, source) in [
            (
                DocumentKind::Typst,
                "é Estmates #emph[hidden #strong[typo]] continous.",
            ),
            (
                DocumentKind::Tex,
                r"é Estmates \emph{hidden \textbf{typo}} continous.",
            ),
        ] {
            let selection = options_selection(
                source,
                kind,
                InputOptions {
                    protected_content: ProtectedContent::Split,
                    formatted_text: FormattedText::Skip,
                    ..Default::default()
                },
            );
            assert_eq!(texts(&selection), ["é Estmates", "continous."], "{source}");
            let edits = selection
                .map_corrections(source, &["é Estimates".into(), "continuous.".into()])
                .unwrap();
            assert_eq!(
                apply(source, &edits),
                source
                    .replace("Estmates", "Estimates")
                    .replace("continous", "continuous")
            );
            assert!(
                selection
                    .map_corrections(source, &["é Estmates".into(), "Now continous.".into()])
                    .is_err()
            );
        }
    }

    #[test]
    fn formatting_skip_preserves_math_and_literal_scanner_boundaries() {
        for (kind, source) in [
            (DocumentKind::Typst, "A estimate $\"secret typo\"$ holds."),
            (
                DocumentKind::Tex,
                r"A estimate \[x + \text{secret typo}\] holds.",
            ),
        ] {
            let selection = options_selection(
                source,
                kind,
                InputOptions {
                    formatted_text: FormattedText::Skip,
                    ..Default::default()
                },
            );
            assert!(
                selection
                    .fragments
                    .iter()
                    .all(|fragment| !fragment.text.contains("secret")),
                "{source}"
            );
            let outputs: Vec<_> = selection
                .fragments
                .iter()
                .map(|fragment| fragment.text.replace("A estimate", "An estimate"))
                .collect();
            let edits = selection.map_corrections(source, &outputs).unwrap();
            assert_eq!(
                apply(source, &edits),
                source.replace("A estimate", "An estimate")
            );
        }
        let source = r"A estimate \verb|\emph{secret typo}| holds.";
        let selection = options_selection(
            source,
            DocumentKind::Tex,
            InputOptions {
                formatted_text: FormattedText::Skip,
                ..Default::default()
            },
        );
        assert_eq!(texts(&selection), ["A estimate x holds."]);
    }
    #[test]
    fn long_selected_paragraph_maps_local_chunks_without_global_quadratic_diff() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let sentence = "The result are stable. ";
            let prose = sentence.repeat(1000);
            assert!(prose.len() > 16_384);
            let body = if kind.is_typst() {
                format!("#emph[{prose}We estimate $u$ safely.]")
            } else {
                format!("\\textbf{{{prose}We estimate $u$ safely.}}")
            };
            let source = format!("Outside are unchanged.\n\n{body}\n\nAfter are unchanged.");
            let selection = Selection::new(&source, kind, selected(&source, &body)).unwrap();
            assert_eq!(selection.fragments.len(), 1);
            let text = &selection.fragments[0].text;
            let mut byte = 0;
            let chunks: Vec<_> = text
                .split_inclusive(". ")
                .map(|piece| {
                    let range = byte..byte + piece.len();
                    byte = range.end;
                    ChunkCorrection {
                        fragment: 0,
                        range,
                        corrected: piece.replace("result are", "result is"),
                    }
                })
                .collect();
            assert!(chunks.len() > 1000);
            let edits = selection.map_chunk_corrections(&source, &chunks).unwrap();
            assert_eq!(edits.len(), 1000);
            assert_eq!(
                apply(&source, &edits),
                source.replace("result are", "result is")
            );
            assert!(
                diff(text, &text.replace("result are", "result is")).is_err(),
                "the bounded global LCS must not grow with paper size"
            );
        }
    }

    #[test]
    fn streamed_chunks_preserve_unicode_protected_ranges_and_snapshot_fences() {
        let source = "Outside.\n\n#emph[Tao’s result $alpha$ are stable. Another result are stable.]\n\nAfter.";
        let selection = Selection::new(
            source,
            DocumentKind::Typst,
            selected(
                source,
                "#emph[Tao’s result $alpha$ are stable. Another result are stable.]",
            ),
        )
        .unwrap();
        let text = &selection.fragments[0].text;
        let boundary = text.find("Another").unwrap();
        let first = ChunkCorrection {
            fragment: 0,
            range: 0..boundary,
            corrected: text[..boundary].replace("are stable", "is stable"),
        };
        let partial = selection.map_chunk(&first).unwrap();
        assert_eq!(
            apply(source, &partial),
            source.replacen("are stable", "is stable", 1)
        );
        let mut invalid = first.clone();
        invalid.range = 0.."Tao".len() + 1; // Inside a curly apostrophe.
        assert!(selection.map_chunk(&invalid).is_err());
        invalid = first.clone();
        invalid.corrected = invalid.corrected.replace(" x ", " changed ");
        assert!(
            selection.map_chunk(&invalid).is_err(),
            "the math placeholder cannot be changed"
        );
        assert!(
            selection
                .map_chunk_corrections(source, &[first.clone(), first.clone()])
                .is_err()
        );
        assert!(
            selection
                .map_chunk_corrections(&source.replace("Outside", "Changed"), &[first])
                .is_err()
        );
    }
    #[test]
    fn cached_word_fences_bound_local_mapping_work_and_preserve_strict_edges() {
        let words = 32_768;
        let text = format!("{}safe", "word ".repeat(words));
        let mut source = Vec::new();
        let mut builder = Builder::new(0);
        builder.has_word = true;
        for (at, ch) in text.chars().enumerate() {
            if at < words * 5 && at % 5 == 1 {
                source.push('#'); // Omitted markup splits this lexical word.
            }
            let position = source.len();
            source.push(ch);
            builder.literal(ch, position).unwrap();
        }
        let mut fragments = Vec::new();
        builder.finish(source.len(), &mut fragments).unwrap();
        let fragment = fragments.pop().unwrap();
        assert_eq!(fragment.discontinuous_words.len(), words);
        let last = fragment.origins.len() - 4;
        WORD_FENCE_PROBES.with(|probes| probes.set(0));
        assert_eq!(
            fragment.map_edit(&source, last..last + 4).unwrap(),
            source.len() - 4..source.len()
        );
        assert!(
            WORD_FENCE_PROBES.with(|probes| probes.get()) <= 18,
            "a local edit must inspect logarithmically many cached spans, never scan the paragraph"
        );
        assert!(fragment.map_edit(&source, 1..4).is_err());
        assert!(
            fragment.map_edit(&source, 1..1).is_err(),
            "interior insertions must retain the word fence"
        );
        assert_eq!(fragment.map_edit(&source, 0..0).unwrap(), 0..0);
        assert_eq!(fragment.map_edit(&source, 4..4).unwrap(), 5..5);
        assert!(
            fragment
                .map_edit(
                    &source,
                    fragment.origins.len() + 1..fragment.origins.len() + 1
                )
                .is_err()
        );
    }
    #[test]
    fn tex_long_formatted_paragraph_with_parenthesized_math_has_exact_safe_projection() {
        let prose = format!(
            "She go to school every day. {}We estimate \\(u\\).",
            "This is fine. ".repeat(180)
        );
        let body = format!("\\textbf{{{prose}}}");
        let source = format!(
            "\\documentclass{{article}}\n\\begin{{document}}\nHe go to school.\n{body}\nUnchanged afterward.\n\\end{{document}}\n% native undo sentinel"
        );
        let selection =
            Selection::new(&source, DocumentKind::Tex, selected(&source, &body)).unwrap();
        let expected = prose.replace(r"\(u\)", "x");
        assert_eq!(texts(&selection), [expected.as_str()]);
        let mut byte = 0;
        let chunks: Vec<_> = expected
            .split_inclusive(". ")
            .map(|part| {
                let range = byte..byte + part.len();
                byte = range.end;
                ChunkCorrection {
                    fragment: 0,
                    range,
                    corrected: part.replace("She go ", "She goes "),
                }
            })
            .collect();
        let edits = selection.map_chunk_corrections(&source, &chunks).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            apply(&source, &edits),
            source.replace("She go ", "She goes ")
        );
    }
}

#[cfg(test)]
mod configurable_prose_tests {
    use super::*;
    use crate::writing::prose_rules::{TexArgumentRole, TexCommandRule, TexProseRules};
    #[test]
    fn proposal_validation_keeps_the_original_custom_command_rules() {
        let source = r"The \myprose{reslt} is correct.";
        let mut rules = TexProseRules::default();
        rules.commands.push(TexCommandRule {
            name: "myprose".into(),
            arguments: vec![TexArgumentRole::InlineProse],
            optional_arguments: vec![],
        });
        let selection = Selection::with_prose_rules(
            source,
            DocumentKind::Tex,
            0..source.chars().count(),
            InputOptions::default(),
            Some(&rules),
        )
        .unwrap();
        assert_eq!(selection.fragments[0].text, "The reslt is correct.");
        let edits = selection
            .map_corrections(source, &["The result is correct.".into()])
            .unwrap();
        assert_eq!(edits.len(), 1);
        let mut result = source.to_owned();
        let edit = &edits[0];
        // This ASCII fixture makes the scalar and byte coordinates identical.
        result.replace_range(edit.range.clone(), &edit.replacement);
        assert_eq!(result, r"The \myprose{result} is correct.");
    }
}
