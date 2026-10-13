//! Mask equation syntax while retaining prose commands and scalar coordinates.
//! Uses the same tolerant math scanner as indentation guides; no macro expansion.
use crate::document::DocumentKind;

pub(super) struct TexProse;

impl harper_core::parsers::Parser for TexProse {
    fn parse(&self, source: &[char]) -> Vec<harper_core::Token> {
        let (mask, punctuation) = prose_mask(source);
        let mut tokens = Vec::new();
        let mut punctuation = punctuation.into_iter().peekable();
        let mut cursor = 0;
        for (span, content) in mask.iter_allowed(source) {
            append_gap(&mut tokens, cursor, span.start, &mut punctuation);
            let mut prose = parse_prose(content);
            for token in &mut prose {
                token.span.push_by(span.start);
            }
            tokens.extend(prose);
            cursor = span.end;
        }
        append_gap(&mut tokens, cursor, source.len(), &mut punctuation);
        tokens
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

fn parse_prose(source: &[char]) -> Vec<harper_core::Token> {
    use harper_core::{Span, Token, TokenKind, parsers::Parser};
    let mut tokens = harper_tex::TeX::default().parse(source);
    for token in &mut tokens {
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
            token.kind = TokenKind::Newline(1);
        }
    }
    let boundaries = structural_boundaries(source);
    if boundaries.is_empty() {
        return tokens;
    }
    let mut boundaries = boundaries.into_iter().peekable();
    let mut result = Vec::with_capacity(tokens.len() + boundaries.len());
    for token in tokens {
        while boundaries.peek().is_some_and(|&at| at <= token.span.start) {
            result.push(Token::new(
                Span::empty(boundaries.next().unwrap()),
                TokenKind::ParagraphBreak,
            ));
        }
        result.push(token);
    }
    for at in boundaries {
        result.push(Token::new(Span::empty(at), TokenKind::ParagraphBreak));
    }
    result
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

fn prose_command(name: &[char]) -> bool {
    [
        "section",
        "title",
        "subsection",
        "subsubsection",
        "textbf",
        "textit",
        "emph",
        "author",
        "part",
        "chapter",
        "caption",
    ]
    .iter()
    .any(|candidate| name.iter().copied().eq(candidate.chars()))
}

fn contains_structural_command(source: &[char]) -> bool {
    !structural_boundaries(source).is_empty()
}

fn structural_boundaries(source: &[char]) -> Vec<usize> {
    let mut boundaries = Vec::new();
    let mut cursor = 0;
    while cursor < source.len() {
        if source[cursor] == '%' {
            skip_comment(source, &mut cursor);
        } else if source[cursor] == '\\' {
            let start = cursor;
            let (name, end) = command(source, cursor);
            cursor = end;
            if !structural_command(name) {
                // Hidden technical arguments (for example footnotes) cannot
                // introduce paragraph boundaries in the surrounding sentence.
                // Follow the upstream parser's explicit prose-command policy.
                if name.iter().all(char::is_ascii_alphabetic) && !prose_command(name) {
                    let mut next = cursor;
                    while source.get(next).is_some_and(|ch| ch.is_whitespace()) {
                        next += 1;
                    }
                    if source.get(next) == Some(&'[') {
                        cursor = group_end(source, next, '[', ']');
                    }
                    if let Some((_, end)) = argument(source, cursor) {
                        cursor = end;
                    }
                }
                continue;
            }
            boundaries.push(start);
            let name: String = name.iter().collect();
            if matches!(name.as_str(), "par" | "item" | "maketitle") {
                boundaries.push(cursor);
                continue;
            }
            if source.get(cursor) == Some(&'*') {
                cursor += 1;
            }
            while source.get(cursor).is_some_and(|ch| ch.is_whitespace()) {
                cursor += 1;
            }
            if source.get(cursor) == Some(&'[') {
                cursor = group_end(source, cursor, '[', ']');
                while source.get(cursor).is_some_and(|ch| ch.is_whitespace()) {
                    cursor += 1;
                }
            }
            if source.get(cursor) == Some(&'{') {
                cursor = group_end(source, cursor, '{', '}');
            }
            boundaries.push(cursor);
        } else {
            cursor += 1;
        }
    }
    // Cursor advances past each boundary's full command/argument, so positions
    // are already ordered; deduplicate without an unnecessary sorting pass.
    boundaries.dedup();
    boundaries
}

#[cfg(test)]
pub(super) struct MathMask;

#[cfg(test)]
impl harper_core::Masker for MathMask {
    fn create_mask(&self, original: &[char]) -> harper_core::Mask {
        prose_mask(original).0
    }
}

fn prose_mask(original: &[char]) -> (harper_core::Mask, Vec<MathPunctuation>) {
    let source: String = original.iter().collect();
    let mut allowed = vec![true; original.len()];
    let offsets: Vec<usize> = source.char_indices().map(|(byte, _)| byte).collect();
    let mut punctuation = Vec::new();
    let mut covered = 0;
    for block in crate::editor_math_guides::blocks(&source, DocumentKind::Tex) {
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
    for range in literal_ranges(original) {
        allowed[range].fill(false);
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

fn literal_ranges(source: &[char]) -> Vec<std::ops::Range<usize>> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while cursor < source.len() {
        if source[cursor] == '%' {
            skip_comment(source, &mut cursor);
        } else if source[cursor] == '\\' {
            let start = cursor;
            let (name, end) = command(source, cursor);
            cursor = end;
            if name == ['v', 'e', 'r', 'b'] {
                if source.get(cursor) == Some(&'*') {
                    cursor += 1;
                }
                if let Some(&delimiter) = source.get(cursor) {
                    if matches!(delimiter, '\n' | '\r') {
                        result.push(start..cursor);
                        continue;
                    }
                    cursor += 1;
                    while cursor < source.len()
                        && source[cursor] != delimiter
                        && source[cursor] != '\n'
                    {
                        cursor += 1;
                    }
                    if source.get(cursor) == Some(&delimiter) {
                        cursor += 1;
                    }
                    result.push(start..cursor);
                }
            } else if name == ['b', 'e', 'g', 'i', 'n']
                && let Some((open, end)) = argument(source, cursor)
            {
                let environment: String = source[open + 1..end.saturating_sub(1)].iter().collect();
                if !matches!(
                    environment.as_str(),
                    "verbatim" | "verbatim*" | "lstlisting" | "minted"
                ) {
                    continue;
                }
                let close: Vec<_> = format!("\\end{{{environment}}}").chars().collect();
                cursor = end;
                while cursor < source.len() && !source[cursor..].starts_with(&close) {
                    cursor += 1;
                }
                cursor = (cursor + close.len()).min(source.len());
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
    fn opaque_command_arguments_do_not_create_outer_prose_paragraphs() {
        let source = r"The result\footnote{A note.\par Another note.} is correct.";
        assert!(
            parsed(source)
                .iter()
                .all(|token| !token.kind.is_paragraph_break()),
            "{:?}",
            parsed(source)
        );
        let formatted = r"The result \emph{contains prose\par with another paragraph} afterward.";
        assert!(
            parsed(formatted)
                .iter()
                .any(|token| token.kind.is_paragraph_break()),
            "{:?}",
            parsed(formatted)
        );
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
