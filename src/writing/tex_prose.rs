//! Mask equation syntax while retaining prose commands and scalar coordinates.
//! Uses the same tolerant math scanner as indentation guides; no macro expansion.
use super::prose_rules::{ProseRegion, TexProseRules};
use crate::document::DocumentKind;

static DEFAULT_RULES: std::sync::LazyLock<TexProseRules> =
    std::sync::LazyLock::new(TexProseRules::default);

pub(super) struct TexProse;

impl harper_core::parsers::Parser for TexProse {
    fn parse(&self, source: &[char]) -> Vec<harper_core::Token> {
        Self::parse_with_rules(source, &DEFAULT_RULES, false)
    }
}

impl TexProse {
    pub(super) fn parse_skipping_formatting(source: &[char]) -> Vec<harper_core::Token> {
        Self::parse_with_rules(source, &DEFAULT_RULES, true)
    }

    pub(super) fn parse_with_rules(
        source: &[char],
        rules: &TexProseRules,
        skip_formatted: bool,
    ) -> Vec<harper_core::Token> {
        let (mask, punctuation) = prose_mask(source, skip_formatted, rules);
        // Preserve complete TeX command/group context while excluding math and
        // literal payloads. Parsing isolated allowed spans loses a formatter's
        // closing brace across an equation and can admit its command as prose.
        let mut projected = vec![' '; source.len()];
        for (span, content) in mask.iter_allowed(source) {
            projected[span.start..span.end].copy_from_slice(content);
        }
        let prose = parse_prose(&projected, rules);
        let mut tokens = Vec::new();
        let mut punctuation = punctuation.into_iter().peekable();
        let mut cursor = 0;
        let mut next_token = 0;
        for (span, _) in mask.iter_allowed(source) {
            append_gap(&mut tokens, cursor, span.start, &mut punctuation);
            while prose.get(next_token).is_some_and(|token| {
                token.span.end <= span.start
                    && !(token.span.is_empty() && token.span.start == span.start)
            }) {
                next_token += 1;
            }
            while let Some(token) = prose.get(next_token)
                && token.span.start <= span.end
            {
                let start = token.span.start.max(span.start);
                let end = token.span.end.min(span.end);
                if start < end || (token.span.is_empty() && start == end) {
                    append_admitted_token(&mut tokens, token, source, start, end);
                }
                if token.span.end > span.end {
                    break;
                }
                next_token += 1;
            }
            cursor = span.end;
        }
        append_gap(&mut tokens, cursor, source.len(), &mut punctuation);
        tokens
    }
}

/// Whitespace can straddle a blanked math range. Re-tokenize only its admitted
/// original slice so space/newline counts cannot include the hidden payload.
fn append_admitted_token(
    tokens: &mut Vec<harper_core::Token>,
    token: &harper_core::Token,
    source: &[char],
    start: usize,
    end: usize,
) {
    if (token.span.start != start || token.span.end != end)
        && (token.kind.is_whitespace() || token.kind.is_paragraph_break())
        && source[start..end].iter().all(|ch| ch.is_whitespace())
    {
        use harper_core::{
            Span, Token, TokenKind,
            parsers::{Parser, PlainEnglish},
        };
        let slice = &source[start..end];
        if slice.iter().filter(|&&ch| ch == '\n').count() >= 2 {
            tokens.push(Token::new(Span::new(start, end), TokenKind::ParagraphBreak));
        } else {
            let mut admitted = PlainEnglish.parse(slice);
            for token in &mut admitted {
                token.span.push_by(start);
            }
            tokens.extend(admitted);
        }
    } else {
        let mut admitted = token.clone();
        admitted.span = harper_core::Span::new(start, end);
        normalize_layout_newline(&mut admitted, source);
        tokens.push(admitted);
    }
}

/// Punctuation keeps its original coordinate. The rest of a terminal equation
/// suffix consists only of labels, spacing, comments, and closing syntax, so it
/// must not become the first semantic token of the following sentence.
struct MathPunctuation {
    token: harper_core::Token,
    skip_to: usize,
}

fn append_gap(
    tokens: &mut Vec<harper_core::Token>,
    mut start: usize,
    end: usize,
    punctuation: &mut std::iter::Peekable<std::vec::IntoIter<MathPunctuation>>,
) {
    use harper_core::{Span, Token, TokenKind};
    while punctuation
        .peek()
        .is_some_and(|mark| mark.token.span.start < end)
    {
        let mark = punctuation.next().unwrap();
        if mark.token.span.start < start {
            continue;
        }
        if start < mark.token.span.start {
            tokens.push(Token::new(
                Span::new(start, mark.token.span.start),
                TokenKind::Unlintable,
            ));
        }
        start = mark.skip_to.max(mark.token.span.end);
        tokens.push(mark.token);
    }
    if start < end {
        // An equation or literal code occupies this gap. Keeping an opaque
        // semantic object prevents invented adjacent words and punctuation gaps.
        tokens.push(Token::new(Span::new(start, end), TokenKind::Unlintable));
    }
}

fn parse_prose(source: &[char], rules: &TexProseRules) -> Vec<harper_core::Token> {
    use harper_core::{
        Span, Token, TokenKind,
        parsers::{Parser, PlainEnglish},
    };
    let mut tokens = Vec::new();
    for region in super::prose_rules::regions(source, rules) {
        match region {
            ProseRegion::Prose(range) => {
                let mut prose = PlainEnglish.parse(&source[range.clone()]);
                for token in &mut prose {
                    token.span.push_by(range.start);
                }
                tokens.extend(prose);
            }
            ProseRegion::Opaque(range) if !range.is_empty() => tokens.push(Token::new(
                Span::new(range.start, range.end),
                TokenKind::Unlintable,
            )),
            ProseRegion::Boundary(at) => {
                tokens.push(Token::new(Span::empty(at), TokenKind::ParagraphBreak))
            }
            ProseRegion::Opaque(_) => {}
        }
    }
    // TeX's two/triple ASCII dashes are punctuation, not repeated hyphens.
    let mut result: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut cursor = 0;
    while cursor < tokens.len() {
        if tokens[cursor].kind == TokenKind::Punctuation(harper_core::Punctuation::Hyphen) {
            let mut count = 1;
            while count < 3
                && tokens.get(cursor + count).is_some_and(|token| {
                    token.kind == TokenKind::Punctuation(harper_core::Punctuation::Hyphen)
                        && token.span.start == tokens[cursor + count - 1].span.end
                })
            {
                count += 1;
            }
            if count > 1 {
                result.push(Token::new(
                    Span::new(
                        tokens[cursor].span.start,
                        tokens[cursor + count - 1].span.end,
                    ),
                    TokenKind::Punctuation(if count == 3 {
                        harper_core::Punctuation::EmDash
                    } else {
                        harper_core::Punctuation::EnDash
                    }),
                ));
                cursor += count;
                continue;
            }
        }
        result.push(tokens[cursor].clone());
        cursor += 1;
    }
    result
}

fn normalize_layout_newline(token: &mut harper_core::Token, source: &[char]) {
    if token.kind.is_paragraph_break()
        && token
            .span
            .get_content(source)
            .iter()
            .filter(|&&ch| ch == '\n')
            .count()
            == 1
        && !contains_structural_command(token.span.get_content(source))
    {
        // Harper's TeX mask interprets any newline hidden with a command or
        // indentation as a paragraph. A single source line break is layout.
        token.kind = harper_core::TokenKind::Newline(1);
    }
}

fn structural_command(name: &[char]) -> bool {
    [
        "par",
        "item",
        "begin",
        "end",
        "part",
        "chapter",
        "section",
        "subsection",
        "subsubsection",
        "paragraph",
        "subparagraph",
        "title",
        "maketitle",
    ]
    .iter()
    .any(|candidate| name.iter().copied().eq(candidate.chars()))
}

fn contains_structural_command(source: &[char]) -> bool {
    let mut cursor = 0;
    while cursor < source.len() {
        if source[cursor] == '%' {
            skip_comment(source, &mut cursor);
        } else if source[cursor] == '\\' {
            let (name, end) = command(source, cursor);
            if structural_command(name) {
                return true;
            }
            cursor = end;
        } else {
            cursor += 1;
        }
    }
    false
}

#[cfg(test)]
pub(super) struct MathMask;

#[cfg(test)]
impl harper_core::Masker for MathMask {
    fn create_mask(&self, original: &[char]) -> harper_core::Mask {
        prose_mask(original, false, &DEFAULT_RULES).0
    }
}

fn prose_mask(
    original: &[char],
    skip_formatted: bool,
    rules: &TexProseRules,
) -> (harper_core::Mask, Vec<MathPunctuation>) {
    let source: String = original.iter().collect();
    let mut allowed = vec![true; original.len()];
    let offsets: Vec<usize> = source.char_indices().map(|(byte, _)| byte).collect();
    // Discover opaque environments before scanning mathematics: a literal '$'
    // in a custom code environment must not hide the rest of the document.
    let literals = literal_ranges(original, &source, &offsets, rules);
    let math_source = if literals.is_empty() {
        std::borrow::Cow::Borrowed(source.as_str())
    } else {
        let mut bytes = source.as_bytes().to_vec();
        for range in &literals {
            let start = offsets.get(range.start).copied().unwrap_or(source.len());
            let end = offsets.get(range.end).copied().unwrap_or(source.len());
            bytes[start..end].fill(b' ');
        }
        // Replacements cover complete original characters, preserving byte
        // positions even for non-ASCII literal delimiters and payloads.
        std::borrow::Cow::Owned(
            String::from_utf8(bytes).expect("whole-character masking preserves UTF-8"),
        )
    };
    let mut punctuation = Vec::new();
    let mut covered = 0;
    for block in crate::editor_math_guides::blocks(&math_source, DocumentKind::Tex) {
        // Outer blocks precede nested blocks. Process each source character once.
        if block.range.start < covered {
            continue;
        }
        covered = block.range.end;
        let start = offsets.partition_point(|&byte| byte < block.range.start);
        let end = offsets.partition_point(|&byte| byte < block.range.end);
        allowed[start..end].fill(false);
        reveal_text(&original[start..end], &mut allowed[start..end]);
        punctuation.extend(math_punctuation(&original[start..end], start));
    }
    // Literal payloads must remain opaque even when they contain math delimiters
    // or a text command inside an equation's prose annotation.
    for range in &literals {
        allowed[range.clone()].fill(false);
    }
    if skip_formatted {
        let formatting = formatting_ranges(original, &literals);
        for range in &formatting {
            allowed[range.clone()].fill(false);
        }
        // A skipped prose command inside an equation must not leak its own
        // punctuation back through the equation punctuation adapter.
        let mut index = 0;
        punctuation.retain(|mark| {
            while formatting
                .get(index)
                .is_some_and(|range| range.end <= mark.token.span.start)
            {
                index += 1;
            }
            !formatting
                .get(index)
                .is_some_and(|range| range.contains(&mark.token.span.start))
        });
    }
    let mut mask = harper_core::Mask::new_blank();
    let mut cursor = 0;
    while cursor < allowed.len() {
        if !allowed[cursor] {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < allowed.len() && allowed[cursor] {
            cursor += 1;
        }
        mask.push_allowed(harper_core::Span::new(start, cursor));
    }
    (mask, punctuation)
}

/// Recognized inline prose formatters, using the same escaped-brace/comment
/// scanner as TeX prose admission. Literal environments and verbatim commands
/// cannot introduce fake formatting ranges.
fn formatting_ranges(
    source: &[char],
    literals: &[std::ops::Range<usize>],
) -> Vec<std::ops::Range<usize>> {
    let mut result = Vec::new();
    let mut cursor = 0;
    let mut literal = 0;
    while cursor < source.len() {
        while literals
            .get(literal)
            .is_some_and(|range| range.end <= cursor)
        {
            literal += 1;
        }
        if let Some(range) = literals
            .get(literal)
            .filter(|range| range.contains(&cursor))
        {
            cursor = range.end;
            continue;
        }
        if source[cursor] == '%' {
            skip_comment(source, &mut cursor);
        } else if source[cursor] == '\\' {
            let start = cursor;
            let (name, end) = command(source, cursor);
            cursor = end;
            if ![
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
            ]
            .iter()
            .any(|candidate| name.iter().copied().eq(candidate.chars()))
            {
                continue;
            }
            let mut next = cursor;
            if source.get(next) == Some(&'*') {
                next += 1;
            }
            while source.get(next).is_some_and(|ch| ch.is_whitespace()) {
                next += 1;
            }
            if source.get(next) == Some(&'[') {
                next = group_end(source, next, '[', ']');
            }
            while source.get(next).is_some_and(|ch| ch.is_whitespace()) {
                next += 1;
            }
            if source.get(next) == Some(&'{') {
                let end = formatting_group_end(source, next, literals);
                result.push(start..end);
                cursor = end;
            }
        } else {
            cursor += 1;
        }
    }
    result
}

fn formatting_group_end(
    source: &[char],
    start: usize,
    literals: &[std::ops::Range<usize>],
) -> usize {
    let mut cursor = start + 1;
    let mut depth = 1;
    let mut literal = literals.partition_point(|range| range.end <= cursor);
    while cursor < source.len() {
        while literals
            .get(literal)
            .is_some_and(|range| range.end <= cursor)
        {
            literal += 1;
        }
        if let Some(range) = literals
            .get(literal)
            .filter(|range| range.contains(&cursor))
        {
            cursor = range.end;
            continue;
        }
        match source[cursor] {
            '\\' => cursor = (cursor + 2).min(source.len()),
            '%' => skip_comment(source, &mut cursor),
            '{' => {
                depth += 1;
                cursor += 1;
            }
            '}' => {
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    break;
                }
            }
            _ => cursor += 1,
        }
    }
    cursor
}

fn command(source: &[char], cursor: usize) -> (&[char], usize) {
    let start = cursor + 1;
    let mut end = start;
    while source.get(end).is_some_and(|ch| ch.is_ascii_alphabetic()) {
        end += 1;
    }
    if end == start {
        end = (end + 1).min(source.len());
    }
    (&source[start..end], end)
}

fn skip_comment(source: &[char], cursor: &mut usize) {
    while *cursor < source.len() && source[*cursor] != '\n' {
        *cursor += 1;
    }
}

fn group_end(source: &[char], start: usize, open: char, close: char) -> usize {
    let mut cursor = start + 1;
    let mut depth = 1;
    while cursor < source.len() {
        match source[cursor] {
            '\\' => cursor = (cursor + 2).min(source.len()),
            '%' => skip_comment(source, &mut cursor),
            ch if ch == open => {
                depth += 1;
                cursor += 1;
            }
            ch if ch == close => {
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    break;
                }
            }
            _ => cursor += 1,
        }
    }
    cursor
}

fn argument(source: &[char], mut cursor: usize) -> Option<(usize, usize)> {
    while source.get(cursor).is_some_and(|ch| ch.is_whitespace()) {
        cursor += 1;
    }
    (source.get(cursor) == Some(&'{')).then(|| (cursor, group_end(source, cursor, '{', '}')))
}

fn literal_ranges(
    source: &[char],
    text: &str,
    byte_offsets: &[usize],
    rules: &TexProseRules,
) -> Vec<std::ops::Range<usize>> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while cursor < source.len() {
        if source[cursor] == '%' {
            skip_comment(source, &mut cursor);
        } else if source[cursor] == '\\' {
            let start = cursor;
            if let Some(end) =
                crate::editor_math_guides::tex_inline_literal_end(text, byte_offsets[start])
            {
                cursor = byte_offsets.partition_point(|&byte| byte < end);
                result.push(start..cursor);
                continue;
            }
            let (name, end) = command(source, cursor);
            cursor = end;
            if name == ['b', 'e', 'g', 'i', 'n']
                && let Some((open, end)) = argument(source, cursor)
            {
                let environment: String = source[open + 1..end.saturating_sub(1)].iter().collect();
                if !rules.environments.iter().any(|rule| {
                    rule.name == environment
                        && !matches!(
                            rule.body,
                            super::prose_rules::TexArgumentRole::InlineProse
                                | super::prose_rules::TexArgumentRole::SeparateProse
                        )
                }) {
                    continue;
                }
                if matches!(
                    environment.as_str(),
                    "verbatim" | "verbatim*" | "Verbatim" | "lstlisting" | "minted" | "comment"
                ) {
                    // Actual verbatim-like environments do not nest: a printed
                    // \begin inside their payload is just literal source.
                    let close: Vec<_> = format!("\\end{{{environment}}}").chars().collect();
                    cursor = end;
                    while cursor < source.len() && !source[cursor..].starts_with(&close) {
                        cursor += 1;
                    }
                    cursor = (cursor + close.len()).min(source.len());
                } else {
                    let environment: Vec<_> = environment.chars().collect();
                    cursor = super::prose_rules::environment_end(
                        source,
                        end,
                        source.len(),
                        &environment,
                    );
                }
                result.push(start..cursor);
            }
        } else {
            cursor += 1;
        }
    }
    result
}

fn math_punctuation(source: &[char], offset: usize) -> Vec<MathPunctuation> {
    use harper_core::{Punctuation, Span, Token, TokenKind};
    let mut result = Vec::new();
    let mut cursor = if source.starts_with(&['$', '$']) {
        2
    } else if source.first() == Some(&'$') {
        1
    } else {
        2
    };
    let mut end = source.len();
    let mut closed = false;
    if source.starts_with(&['\\', 'b']) {
        let (_, command_end) = command(source, 0);
        if let Some((open, argument_end)) = argument(source, command_end) {
            cursor = argument_end;
            let environment: String = source[open + 1..argument_end.saturating_sub(1)]
                .iter()
                .collect();
            let close: Vec<_> = format!("\\end{{{environment}}}").chars().collect();
            if source.ends_with(&close) {
                end -= close.len();
                closed = true;
            }
        } else {
            cursor = command_end;
        }
    } else {
        let delimiter = if source.first() == Some(&'$') {
            &source[..cursor]
        } else if source.get(1) == Some(&'[') {
            &['\\', ']'][..]
        } else {
            &['\\', ')'][..]
        };
        if source.ends_with(delimiter) && end >= cursor + delimiter.len() {
            end -= delimiter.len();
            closed = true;
        }
    }
    if !closed {
        return result;
    }
    let mut nesting = Vec::new();
    let mut environments = Vec::new();
    let mut opaque_environments = 0;
    let mut candidate = None;
    while cursor < end {
        let ch = source[cursor];
        if ch.is_whitespace() {
            cursor += 1;
        } else if ch == '%' {
            skip_comment(source, &mut cursor);
        } else if ch == '\\' {
            let (name, command_end) = command(source, cursor);
            cursor = command_end;
            let name: String = name.iter().collect();
            match name.as_str() {
                "\\" if nesting.is_empty() && opaque_environments == 0 => {
                    if let Some(token) = candidate.take() {
                        result.push(MathPunctuation {
                            token,
                            skip_to: offset + cursor,
                        });
                    }
                }
                "," | ";" | ":" | "!" | " " | "quad" | "qquad" | "enspace" | "thinspace"
                | "medspace" | "thickspace" | "negthinspace" | "negmedspace" | "negthickspace"
                | "notag" | "nonumber" => {}
                "left" | "right" | "middle" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr"
                | "bigm" | "Bigl" | "Bigr" | "Bigm" | "biggl" | "biggr" | "biggm" | "Biggl"
                | "Biggr" | "Biggm" => {
                    candidate = None;
                    while source.get(cursor).is_some_and(|ch| ch.is_whitespace()) {
                        cursor += 1;
                    }
                    // A delimiter dot is invisible syntax. Consume exactly one;
                    // a second dot may be genuine sentence punctuation.
                    if source.get(cursor) == Some(&'.') {
                        cursor += 1;
                    }
                }
                "label" | "tag" | "hspace" | "vspace" => {
                    if source.get(cursor) == Some(&'*') {
                        cursor += 1;
                    }
                    if let Some((_, argument_end)) = argument(source, cursor) {
                        cursor = argument_end;
                    }
                }
                "begin" | "end" => {
                    if let Some((open, argument_end)) = argument(source, cursor) {
                        let environment: String = source[open + 1..argument_end.saturating_sub(1)]
                            .iter()
                            .collect();
                        if name == "begin" {
                            let opaque = !matches!(
                                environment.as_str(),
                                "aligned" | "alignedat" | "gathered" | "split"
                            );
                            opaque_environments += usize::from(opaque);
                            environments.push(opaque);
                        } else if let Some(opaque) = environments.pop() {
                            opaque_environments -= usize::from(opaque);
                        }
                        cursor = argument_end;
                    }
                }
                _ => {
                    candidate = None;
                }
            }
        } else if ch == '.' && source.get(cursor + 1) == Some(&'.') {
            // A maximal ASCII dot run is mathematical ellipsis/notation, not
            // its last dot pretending to be a prose sentence terminator.
            while source.get(cursor) == Some(&'.') {
                cursor += 1;
            }
            candidate = None;
        } else {
            match ch {
                '(' | '[' | '{' => {
                    nesting.push(ch);
                    candidate = None;
                }
                ')' | ']' | '}' => {
                    nesting.pop();
                    candidate = None;
                }
                ',' | ';' | ':' | '.'
                    if nesting.is_empty()
                        && opaque_environments == 0
                        && !(ch == '.'
                            && source
                                .get(cursor.wrapping_sub(1))
                                .is_some_and(char::is_ascii_digit)
                            && source.get(cursor + 1).is_some_and(char::is_ascii_digit)) =>
                {
                    let punctuation = match ch {
                        ',' => Punctuation::Comma,
                        ';' => Punctuation::Semicolon,
                        ':' => Punctuation::Colon,
                        '.' => Punctuation::Period,
                        _ => unreachable!(),
                    };
                    candidate = Some(Token::new(
                        Span::new(offset + cursor, offset + cursor + 1),
                        TokenKind::Punctuation(punctuation),
                    ));
                }
                _ => {
                    candidate = None;
                }
            }
            cursor += 1;
        }
    }
    if let Some(token) = candidate {
        result.push(MathPunctuation {
            token,
            skip_to: offset + source.len(),
        });
    }
    result
}

fn reveal_text(original: &[char], allowed: &mut [bool]) {
    let mut cursor = 0;
    while cursor < original.len() {
        if original[cursor] == '%' {
            while cursor < original.len() && original[cursor] != '\n' {
                cursor += 1;
            }
        } else if original[cursor] == '\\' {
            cursor += 1;
            let start = cursor;
            while cursor < original.len() && original[cursor].is_ascii_alphabetic() {
                cursor += 1;
            }
            if cursor == start {
                cursor = (cursor + 1).min(original.len());
                continue;
            }
            let name: String = original[start..cursor].iter().collect();
            if !matches!(
                name.as_str(),
                "text"
                    | "textrm"
                    | "textnormal"
                    | "textbf"
                    | "textit"
                    | "textsf"
                    | "texttt"
                    | "mbox"
                    | "hbox"
                    | "intertext"
                    | "shortintertext"
            ) {
                continue;
            }
            while cursor < original.len() && original[cursor].is_whitespace() {
                cursor += 1;
            }
            if original.get(cursor) != Some(&'{') {
                continue;
            }
            cursor += 1;
            let body = cursor;
            let mut depth = 1;
            while cursor < original.len() {
                match original[cursor] {
                    '\\' => cursor = (cursor + 2).min(original.len()),
                    '%' => {
                        while cursor < original.len() && original[cursor] != '\n' {
                            cursor += 1;
                        }
                    }
                    '{' => {
                        depth += 1;
                        cursor += 1;
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        cursor += 1;
                    }
                    _ => cursor += 1,
                }
            }
            // Preserve nested prose commands for Harper's normal TeX parser.
            allowed[body..cursor].fill(true);
            cursor = (cursor + 1).min(original.len());
        } else {
            cursor += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harper_core::Masker;

    fn parsed(source: &str) -> Vec<harper_core::Token> {
        use harper_core::parsers::Parser;
        TexProse.parse(&source.chars().collect::<Vec<_>>())
    }

    #[test]
    fn clipped_math_whitespace_keeps_only_real_paragraph_boundaries() {
        use harper_core::{Span, Token, TokenKind};

        let source: Vec<_> = "\n\n hidden \n    \\emph{".chars().collect();
        let paragraph = Token::new(Span::new(0, source.len()), TokenKind::ParagraphBreak);
        let mut tokens = Vec::new();
        append_admitted_token(&mut tokens, &paragraph, &source, 0, 2);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::ParagraphBreak);
        assert_eq!(tokens[0].span, Span::new(0, 2));

        tokens.clear();
        let single_newline = source.iter().rposition(|&ch| ch == '\n').unwrap();
        append_admitted_token(
            &mut tokens,
            &paragraph,
            &source,
            single_newline,
            source.len(),
        );
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Newline(1));
        assert_eq!(tokens[0].span, Span::new(single_newline, source.len()));

        tokens.clear();
        let structural = Token::new(Span::empty(single_newline), TokenKind::ParagraphBreak);
        append_admitted_token(
            &mut tokens,
            &structural,
            &source,
            single_newline,
            single_newline,
        );
        assert_eq!(tokens, [structural]);
    }

    #[test]
    fn manual_formatting_ranges_balance_around_comments_escapes_and_literal_braces() {
        for body in [
            r"secret \textbf{nested typo}",
            r"secret \{ escaped \} typo",
            "secret % } ignored\n typo",
            r"secret \verb|}| hidden typo",
        ] {
            let source = format!("é Before \\emph{{{body}}} after.");
            let chars: Vec<_> = source.chars().collect();
            let offsets: Vec<_> = source.char_indices().map(|(byte, _)| byte).collect();
            let literals = literal_ranges(&chars, &source, &offsets, &DEFAULT_RULES);
            let start = source[..source.find("\\emph").unwrap()].chars().count();
            let end = source[..source.rfind('}').unwrap() + 1].chars().count();
            assert_eq!(
                formatting_ranges(&chars, &literals),
                std::iter::once(start..end).collect::<Vec<_>>(),
                "{source}"
            );
            let tokens = TexProse::parse_skipping_formatting(&chars);
            assert!(
                !tokens.iter().any(|token| token.kind.is_word()
                    && token
                        .span
                        .get_content(&chars)
                        .iter()
                        .copied()
                        .eq("typo".chars())),
                "{source}"
            );
        }
    }

    #[test]
    fn display_math_retains_sentence_punctuation_and_scalar_locations() {
        use harper_core::{Punctuation, TokenKind};
        for (open, close) in [
            ("\\[", "\\]"),
            ("$$", "$$"),
            ("\\begin{align*}", "\\end{align*}"),
        ] {
            for (mark, punctuation) in [
                (',', Punctuation::Comma),
                (';', Punctuation::Semicolon),
                ('.', Punctuation::Period),
                (':', Punctuation::Colon),
            ] {
                let source = format!(
                    "é The estimate is\n{open}\n  f(x)=3.14{mark} \\label{{eq:period.name}} % ignored ?\n{close}\nwhere the bound holds."
                );
                let expected = source[..source.find(&format!("{mark} \\label")).unwrap()]
                    .chars()
                    .count();
                let tokens = parsed(&source);
                assert!(
                    tokens.iter().any(|token| {
                        token.span.start == expected
                            && token.span.end == expected + 1
                            && token.kind == TokenKind::Punctuation(punctuation)
                    }),
                    "{open} {mark}: {tokens:?}"
                );
                assert!(tokens.iter().any(|token| token.kind.is_unlintable()));
                assert!(!tokens.iter().any(|token| token.kind.is_paragraph_break()));
            }
        }
    }

    #[test]
    fn terminal_equation_syntax_does_not_hide_the_next_sentence() {
        use harper_core::{Document, TokenStringExt};
        let source = "The estimate is \\[x=1. \\label{eq:bound}\\]\nthere are several terms in the expression.";
        let document = Document::new_curated(source, &TexProse);
        let sentences: Vec<_> = document.iter_sentences().collect();
        assert_eq!(sentences.len(), 2, "{sentences:?}");
        let first = sentences[1].first_non_whitespace().unwrap();
        assert!(first.kind.is_word(), "{sentences:?}");
        assert_eq!(first.get_str(document.get_source()), "there");
    }

    #[test]
    fn aligned_row_punctuation_is_preserved_without_matrix_punctuation() {
        use harper_core::{Punctuation, TokenKind};
        let source = r"We have \[\begin{aligned}x &= 1, \\ y &= 2; \end{aligned}\] where the estimate holds.";
        let tokens = parsed(source);
        for punctuation in [Punctuation::Comma, Punctuation::Semicolon] {
            assert!(
                tokens
                    .iter()
                    .any(|token| { token.kind == TokenKind::Punctuation(punctuation) }),
                "{tokens:?}"
            );
        }
        let matrix = r"We have \[\begin{pmatrix}x, \\ y.\end{pmatrix}\] where the estimate holds.";
        let punctuation: Vec<_> = parsed(matrix)
            .into_iter()
            .filter(|token| matches!(token.kind, TokenKind::Punctuation(_)))
            .collect();
        assert_eq!(punctuation.len(), 1, "{punctuation:?}");
        assert_eq!(punctuation[0].span.start, matrix.chars().count() - 1);
    }

    #[test]
    fn equation_content_punctuation_does_not_become_sentence_punctuation() {
        use harper_core::TokenKind;
        for body in [
            r"f(1, 2) = 3.14",
            r"\frac{a,b}{c.d} = x",
            r"x\,y\!z",
            r"n!",
            r"x?",
            r"\left. x \right.",
            r"\big. x \Bigg.",
            r"\bigl. x \Biggr.",
            "x=y % punctuation .?!,;:\n",
            r"x=y\label{eq:with.period}",
        ] {
            let source = format!("The estimate \\[{body}\\] is valid.");
            let punctuation: Vec<_> = parsed(&source)
                .into_iter()
                .filter(|token| matches!(token.kind, TokenKind::Punctuation(_)))
                .collect();
            assert_eq!(punctuation.len(), 1, "{body}: {punctuation:?}");
            assert_eq!(punctuation[0].span.start, source.chars().count() - 1);
        }
    }

    #[test]
    fn explicit_period_after_invisible_delimiter_is_preserved() {
        use harper_core::{Punctuation, TokenKind};
        let source = "The estimate is \\[\\left. x \\right..\\]\nNext sentence.";
        let expected = source[..source.find("..").unwrap()].chars().count() + 1;
        let periods: Vec<_> = parsed(source)
            .into_iter()
            .filter(|token| token.kind == TokenKind::Punctuation(Punctuation::Period))
            .collect();
        assert_eq!(periods.len(), 2, "{periods:?}");
        assert_eq!(periods[0].span.start, expected);
    }

    #[test]
    fn punctuation_on_an_earlier_row_does_not_hide_the_final_equation_row() {
        let source = r"We have \[x=1, \\ y=2\] where the estimate holds.";
        let chars: Vec<_> = source.chars().collect();
        let tokens = parsed(source);
        let comma = tokens
            .iter()
            .position(|token| token.kind.is_comma())
            .unwrap();
        let where_token = tokens
            .iter()
            .position(|token| token.get_str(&chars) == "where")
            .unwrap();
        assert!(
            tokens[comma + 1..where_token].iter().any(|token| {
                token.kind.is_unlintable() && token.get_str(&chars).contains("y=2")
            }),
            "{tokens:?}"
        );
    }

    #[test]
    fn ellipsis_is_opaque_but_an_explicit_period_after_dots_is_preserved() {
        use harper_core::{Punctuation, TokenKind};
        for body in ["x..", "x...", "x...."] {
            let source = format!("The sequence \\[{body}\\] continues with several terms.");
            let periods: Vec<_> = parsed(&source)
                .into_iter()
                .filter(|token| token.kind == TokenKind::Punctuation(Punctuation::Period))
                .collect();
            assert_eq!(periods.len(), 1, "{body}: {periods:?}");
            assert_eq!(periods[0].span.start, source.chars().count() - 1);
        }
        let source = r"The sequence \[x,\ldots.\] Next sentence.";
        let periods: Vec<_> = parsed(source)
            .into_iter()
            .filter(|token| token.kind == TokenKind::Punctuation(Punctuation::Period))
            .collect();
        assert_eq!(periods.len(), 2, "{periods:?}");
    }

    #[test]
    fn incomplete_math_does_not_invent_sentence_punctuation() {
        for source in [
            r"The estimate follows from \[x=1.",
            r"The estimate follows from $$x=1,",
            r"The estimate follows from \begin{align}x=1;",
            r"The estimate follows from \begin{align}x=1;\end{aligned}",
        ] {
            assert!(
                parsed(source)
                    .iter()
                    .all(|token| { !matches!(token.kind, harper_core::TokenKind::Punctuation(_)) }),
                "{source}: {:?}",
                parsed(source)
            );
        }
    }

    #[test]
    fn incomplete_verb_at_a_line_break_does_not_swallow_following_prose() {
        for newline in ["\n", "\r\n"] {
            let source = format!(
                "First line.{newline}\\verb{newline}The misspelld word remains prose.{newline}"
            );
            let chars: Vec<_> = source.chars().collect();
            assert!(
                parsed(&source)
                    .iter()
                    .any(|token| { token.kind.is_word() && token.get_str(&chars) == "misspelld" }),
                "{source:?}: {:?}",
                parsed(&source)
            );
        }
    }

    #[test]
    fn literal_math_openers_cannot_mask_following_prose() {
        for source in [
            "\\verb|\\[fake|\nThe misspelld word remains prose.",
            "\\begin{minted}{text}\n$ unclosed math\n\\end{minted}\nThe misspelld word remains prose.",
        ] {
            let chars: Vec<_> = source.chars().collect();
            assert!(
                parsed(source)
                    .iter()
                    .any(|token| { token.kind.is_word() && token.get_str(&chars) == "misspelld" }),
                "{source:?}: {:?}",
                parsed(source)
            );
        }
    }

    #[test]
    fn incomplete_verb_preserves_real_math_on_the_following_line() {
        for newline in ["\n", "\r\n"] {
            let source = format!(
                "\\verb{newline}\\[ nonwordxyz + \\text{{This is is readable.}} \\]{newline}The misspelld word remains prose."
            );
            let chars: Vec<_> = source.chars().collect();
            let tokens = parsed(&source);
            assert!(
                tokens.iter().all(|token| {
                    !token.kind.is_word() || token.get_str(&chars) != "nonwordxyz"
                }),
                "{source:?}: {tokens:?}"
            );
            for word in ["readable", "misspelld"] {
                assert!(
                    tokens
                        .iter()
                        .any(|token| { token.kind.is_word() && token.get_str(&chars) == word }),
                    "{word} must remain checked: {source:?}: {tokens:?}"
                );
            }
        }
    }

    #[test]
    fn footnotes_and_nested_spaced_formatting_admit_prose() {
        for source in [
            r"The result\footnote{A misspelld note.} is correct.",
            r"The \textbf {result \emph{contains misspelld words}} is correct.",
            r"The \href {https://example.invalid}{misspelld \emph{visible words}} is correct.",
        ] {
            let chars: Vec<_> = source.chars().collect();
            let tokens = parsed(source);
            assert!(
                tokens
                    .iter()
                    .any(|token| token.kind.is_word() && token.get_str(&chars) == "misspelld"),
                "{source}: {tokens:?}"
            );
        }
    }

    #[test]
    fn separate_arguments_have_boundaries_on_both_sides_and_keep_scalar_locations() {
        use harper_core::{Document, TokenStringExt};
        let source = "é The result\\footnote{The note is useful.} is correct.";
        let chars: Vec<_> = source.chars().collect();
        let tokens = parsed(source);
        let note_start = source[..source.find("The note").unwrap()].chars().count();
        let after_note = source[..source.find(" is correct").unwrap()]
            .chars()
            .count();
        assert!(
            tokens
                .iter()
                .any(|token| token.kind.is_word() && token.span.start == note_start)
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.kind.is_paragraph_break() && token.span.start < note_start)
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.kind.is_paragraph_break() && token.span.start == after_note)
        );
        assert!(
            tokens
                .windows(2)
                .all(|pair| pair[0].span.end <= pair[1].span.start),
            "{tokens:?}"
        );
        let document = Document::new_curated(source, &TexProse);
        assert!(document.iter_sentences().any(|passage| {
            let words: Vec<_> = passage
                .iter()
                .filter(|token| token.kind.is_word())
                .map(|token| token.get_str(&chars))
                .collect();
            words == ["The", "note", "is", "useful"]
        }));
    }

    #[test]
    fn configurable_command_arguments_preserve_nested_math_and_original_coordinates() {
        use super::super::prose_rules::{TexArgumentRole, TexCommandRule};
        let source =
            "é Before \\mycomment{hidden-id}{Visible \\emph{misspelld} $nonwordxyz$ prose.} after.";
        let chars: Vec<_> = source.chars().collect();
        let mut rules = TexProseRules::default();
        rules.commands.push(TexCommandRule {
            name: "mycomment".into(),
            arguments: vec![TexArgumentRole::Identifier, TexArgumentRole::InlineProse],
            optional_arguments: vec![],
        });
        let tokens = TexProse::parse_with_rules(&chars, &rules, false);
        let expected = source[..source.find("misspelld").unwrap()].chars().count();
        assert!(tokens.iter().any(|token| token.kind.is_word()
            && token.span.start == expected
            && token.get_str(&chars) == "misspelld"));
        assert!(!tokens.iter().any(|token| token.kind.is_word()
            && matches!(token.get_str(&chars).as_str(), "hidden" | "nonwordxyz")));
    }

    #[test]
    fn literal_environment_openers_in_code_do_not_nest() {
        let source = "\\begin{verbatim}\\begin{verbatim} nonwordxyz $\n\\end{verbatim} The misspelld word is visible.";
        let chars: Vec<_> = source.chars().collect();
        let tokens = parsed(source);
        assert!(
            tokens
                .iter()
                .any(|token| token.kind.is_word() && token.get_str(&chars) == "misspelld")
        );
        assert!(
            !tokens
                .iter()
                .any(|token| token.kind.is_word() && token.get_str(&chars) == "nonwordxyz")
        );
    }

    #[test]
    fn custom_literal_environments_cannot_open_math_or_hide_following_prose() {
        use super::super::prose_rules::{TexArgumentRole, TexEnvironmentRule};
        let mut rules = TexProseRules::default();
        rules.environments.push(TexEnvironmentRule {
            name: "customcode".into(),
            body: TexArgumentRole::Literal,
            arguments: vec![],
            optional_arguments: vec![],
        });
        for payload in [
            "é $ unfinished",
            "\\[ fake \\text{hidden}",
            "\\begin{customcode}nested $\\end{customcode} more hidden",
        ] {
            let source = format!(
                "Before \\begin{{customcode}}{payload}\\end{{customcode}} The misspelld word is visible. \\[ x=y \\] After."
            );
            let chars: Vec<_> = source.chars().collect();
            let tokens = TexProse::parse_with_rules(&chars, &rules, false);
            for word in ["misspelld", "After"] {
                assert!(
                    tokens
                        .iter()
                        .any(|token| token.kind.is_word() && token.get_str(&chars) == word),
                    "{source}: {tokens:?}"
                );
            }
            assert!(
                !tokens
                    .iter()
                    .any(|token| token.kind.is_word() && token.get_str(&chars) == "hidden")
            );
        }
    }

    #[test]
    fn balanced_technical_arguments_do_not_leak_as_prose() {
        for source in [
            r"The \href{https://bad.invalid/{nonwordxyz}}{visible prose} is correct.",
            r"The \textcolor [named] {nonwordxyz}{visible prose} is correct.",
            r"The \unknown{nonwordxyz}{nonwordxyz} is correct.",
        ] {
            let chars: Vec<_> = source.chars().collect();
            let tokens = parsed(source);
            assert!(
                !tokens
                    .iter()
                    .any(|token| token.kind.is_word() && token.get_str(&chars) == "nonwordxyz"),
                "{source}: {tokens:?}"
            );
            assert!(
                tokens
                    .iter()
                    .any(|token| token.kind.is_word() && token.get_str(&chars) == "correct"),
                "{source}: {tokens:?}"
            );
        }
    }

    #[test]
    fn formatting_newlines_do_not_create_a_paragraph_but_real_boundaries_do() {
        for source in [
            "A sentence continues\n    \\emph{with prose} afterward.",
            "A sentence with \\emph{\n    ordinary prose} that continues.",
            "A sentence with \\textbf{ordinary\n    prose} that continues.",
        ] {
            let tokens = parsed(source);
            assert!(
                !tokens.iter().any(|token| token.kind.is_paragraph_break()),
                "{source:?}: {tokens:?}"
            );
        }
        for source in [
            "First paragraph.\n\nSecond paragraph.",
            "First paragraph.\\par\nSecond paragraph.",
            "First paragraph.\n\\section{A heading}\nSecond paragraph.",
            "First paragraph.\n\\begin{itemize}\n\\item A list item.\n\\end{itemize}",
        ] {
            assert!(
                parsed(source).iter().any(|token| {
                    token.kind.is_paragraph_break()
                        || matches!(token.kind, harper_core::TokenKind::Newline(count) if count >= 2)
                }),
                "a structural paragraph boundary must survive: {source:?}"
            );
        }
    }

    #[test]
    fn code_payloads_are_not_prose_or_equations() {
        use harper_core::TokenKind;
        for source in [
            r"The code \verb|nonwordxyz \[x.\]| is literal.",
            "The code\n\\begin{verbatim}\nnonwordxyz \\[x.\\]\n\\end{verbatim}\nis literal.",
            "The code\n\\begin{lstlisting}\nnonwordxyz \\[x.\\]\n\\end{lstlisting}\nis literal.",
        ] {
            let chars: Vec<_> = source.chars().collect();
            let tokens = parsed(source);
            assert!(
                !tokens.iter().any(|token| {
                    matches!(token.kind, TokenKind::Word(_))
                        && token.span.get_content(&chars).iter().collect::<String>() == "nonwordxyz"
                }),
                "literal code must remain excluded: {source:?}: {tokens:?}"
            );
        }
    }

    #[test]
    fn inline_code_commands_hide_payloads_without_swallowing_following_prose() {
        for code in [
            r"\lstinline|nonwordxyz|",
            r"\lstinline[language=TeX]!nonwordxyz!",
            r"\lstinline{nonwordxyz}",
            r"\lstinline*nonwordxyz*",
            r"\lstinline§nonwordxyz§",
            r"\lstinline[language={[LaTeX]TeX}]|nonwordxyz|",
            r"\mintinline{tex}|nonwordxyz|",
            r"\mintinline{tex}%nonwordxyz%",
            r"\mintinline{tex} %nonwordxyz%",
            "\\mintinline% header\n{tex}|nonwordxyz|",
            r"\mintinline[breaklines]{tex}{nonwordxyz}",
            r"\mintinline{tex}!nonwordxyz $ \text{nonwordxyz}!",
            "\\lstinline|nonwordxyz\n",
            "\\mintinline{tex}|nonwordxyz\n",
        ] {
            let source = format!("é Before {code} misspelld after.");
            let chars: Vec<_> = source.chars().collect();
            let words: Vec<String> = parsed(&source)
                .iter()
                .filter(|token| token.kind.is_word())
                .map(|token| token.span.get_content(&chars).iter().collect())
                .collect();
            assert!(
                !words.iter().any(|word| word == "nonwordxyz"),
                "{source:?}: {words:?}"
            );
            assert!(
                words.iter().any(|word| word == "misspelld"),
                "{source:?}: {words:?}"
            );
        }
    }

    #[test]
    fn mask_preserves_unicode_offsets_and_nested_groups() {
        let source = "é before \\[ xyz \\text{café \\emph{words} \\{yes\\}} + xyz \\] after";
        let original: Vec<_> = source.chars().collect();
        let mask = MathMask.create_mask(&original);
        let text: String = mask
            .iter_allowed(&original)
            .flat_map(|(_, text)| text.iter())
            .collect();
        assert!(text.contains("café \\emph{words} \\{yes\\}"));
        assert!(!text.contains("xyz"));
        let body_start = source[..source.find("café").unwrap()].chars().count();
        assert!(
            mask.iter_allowed(&original)
                .any(|(span, _)| span.start == body_start)
        );
    }

    #[test]
    fn escaped_commands_comments_and_nested_math_do_not_restore_equation_text() {
        let source = "\\[\\\\text{nonwordxyz} % \\text{nonwordxyz}\n\\begin{aligned} x & y \\text{real prose} \\end{aligned}\\]";
        let original: Vec<_> = source.chars().collect();
        let mask = MathMask.create_mask(&original);
        let text: String = mask
            .iter_allowed(&original)
            .flat_map(|(_, text)| text.iter())
            .collect();
        assert!(!text.contains("nonwordxyz"));
        assert_eq!(text, "real prose");
    }
}
