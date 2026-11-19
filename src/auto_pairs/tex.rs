//! Native TeX pairing uses its lexical index; Typst markup punctuation is text.
use crate::delimiters::TexDelimiterIndex;

pub(super) struct Syntax {
    source: String,
    index: TexDelimiterIndex,
    #[cfg(test)]
    pub(super) builds: usize,
}

impl Syntax {
    pub(super) fn new(source: &str) -> Self {
        Self {
            source: source.into(),
            index: TexDelimiterIndex::new(source),
            #[cfg(test)]
            builds: 1,
        }
    }

    pub(super) fn prepare(&mut self, source: &str) {
        if self.source != source {
            self.source.clear();
            self.source.push_str(source);
            self.index = TexDelimiterIndex::new(source);
            #[cfg(test)]
            {
                self.builds += 1;
            }
        }
    }

    pub(super) fn may_open(&self, byte: usize, open: char) -> bool {
        if self.index.ignored_at_caret(byte) || super::escaped_at(&self.source, byte) {
            return false;
        }
        matches!(open, '(' | '[' | '{') || (open == '$' && !self.index.math_at_caret(byte))
    }

    pub(super) fn command_opener(&self, byte: usize, typed: char) -> Option<char> {
        if !matches!(typed, '[' | '(')
            || byte == 0
            || self.source.as_bytes()[byte - 1] != b'\\'
            || super::escaped_at(&self.source, byte - 1)
            || self.index.ignored_at_caret(byte)
            || self.index.math_at_caret(byte - 1)
        {
            return None;
        }
        Some(if typed == '[' { ']' } else { ')' })
    }

    pub(super) fn command_step(&self, byte: usize, text: &str) -> Option<usize> {
        let span = self.command_closer(byte)?;
        let close = self.source.get(span.clone())?;
        let remaining = &self.source[byte..span.end];
        if text == remaining && (byte == span.start || matches!(close, "\\]" | "\\)")) {
            Some(remaining.chars().count())
        } else if text == "\\" && byte == span.start {
            Some(1)
        } else if byte == span.start
            && matches!(close, "\\]" | "\\)")
            && close.ends_with(text)
            && matches!(text, "]" | ")")
        {
            Some(2)
        } else {
            None
        }
    }

    pub(super) fn command_closer(&self, byte: usize) -> Option<std::ops::Range<usize>> {
        let pair = self.index.pair_at(byte)?;
        (pair[1].contains(&byte)
            && self.source.get(pair[1].clone())?.starts_with('\\')
            && self.source.get(pair[0].clone())?.starts_with('\\'))
        .then_some(pair[1].clone())
    }

    pub(super) fn empty_command(&self, byte: usize) -> bool {
        byte >= 2
            && self.index.pair_at(byte - 1).is_some_and(|pair| {
                pair[0].end == byte
                    && pair[1].start == byte
                    && matches!(
                        (&self.source[pair[0].clone()], &self.source[pair[1].clone()]),
                        ("\\[", "\\]") | ("\\(", "\\)")
                    )
            })
    }

    pub(super) fn may_complete_environment(&self, start: usize) -> bool {
        self.index.environment_needs_closer(start)
    }

    pub(super) fn empty_at(&self, byte: usize) -> bool {
        let before = self.source[..byte].chars().next_back();
        let after = self.source[byte..].chars().next();
        if !super::empty_pair_for(before, after, crate::document::DocumentKind::Tex) {
            return false;
        }
        if byte > 0
            && self
                .index
                .pair_at(byte - 1)
                .is_some_and(|pair| pair[0] == (byte - 1..byte) && pair[1].start == byte)
        {
            return true;
        }
        if self.index.ignored_at_caret(byte) {
            return false;
        }
        if before == Some('$') {
            return self
                .index
                .pair_at(byte)
                .is_some_and(|pair| pair[0].end == byte && pair[1].start == byte)
                || (byte > 0
                    && !self.index.math_at_caret(byte - 1)
                    && !super::escaped_at(&self.source, byte - 1));
        }
        byte > 0 && !super::escaped_at(&self.source, byte - 1)
    }

    pub(super) fn may_step_over(&self, byte: usize, close: char) -> bool {
        if !")]}$".contains(close) || super::escaped_at(&self.source, byte) {
            return false;
        }
        if self
            .index
            .pair_at(byte)
            .is_some_and(|pair| pair[1].contains(&byte))
        {
            return true;
        }
        if self.index.ignored_at_caret(byte) {
            return false;
        }
        ")]}".contains(close)
            || (close == '$'
                && self
                    .index
                    .pair_at(byte)
                    .is_some_and(|pair| pair[1].contains(&byte)))
    }

    pub(super) fn expand_empty_math(&self, byte: usize) -> bool {
        byte > 0
            && self.source[..byte].ends_with('$')
            && self.source[byte..].starts_with('$')
            && !self.source[..byte - 1].ends_with('$')
            && !self.source[byte + 1..].starts_with('$')
            && self.empty_at(byte)
    }
}
