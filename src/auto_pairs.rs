//! Adapt native TextEdit operations, keeping event ordering, selections, IME,
//! and undo in their existing owners. Only explicit typing enables pairing.
mod indentation;
pub(crate) mod provisional;
mod tex;

use std::ops::Range;

use eframe::egui::{
    self, TextBuffer,
    text::{CCursor, CharIndex},
};
use tiptoptyp_core::pairing::{self, TypedAction};
use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

/// Only committed typing is normalized. Preedit and paste remain verbatim.
pub(crate) fn ascii_punctuation(events: &mut [egui::Event]) {
    for event in events {
        if let egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) = event
            && text
                .chars()
                .any(|c| c == '。' || ('\u{ff01}'..='\u{ff5e}').contains(&c))
        {
            *text = text
                .chars()
                .map(|c| match c {
                    '。' => '.',
                    '\u{ff01}'..='\u{ff5e}' => {
                        let ascii = char::from_u32(c as u32 - 0xfee0).unwrap();
                        if ascii.is_ascii_punctuation() {
                            ascii
                        } else {
                            c
                        }
                    }
                    _ => c,
                })
                .collect();
        }
    }
}

pub(crate) struct PairSyntax(Source, Option<tex::Syntax>);

impl Default for PairSyntax {
    fn default() -> Self {
        Self(Source::detached(String::new()), None)
    }
}

impl PairSyntax {
    fn prepare_for(&mut self, source: &str, kind: crate::document::DocumentKind) {
        if kind == crate::document::DocumentKind::Tex {
            if let Some(syntax) = &mut self.1 {
                syntax.prepare(source);
            } else {
                self.1 = Some(tex::Syntax::new(source));
            }
        } else {
            self.prepare(source);
        }
    }

    fn may_open_for(
        &mut self,
        source: &str,
        byte: usize,
        open: char,
        kind: crate::document::DocumentKind,
    ) -> bool {
        if kind == crate::document::DocumentKind::Tex {
            self.1.as_ref().unwrap().may_open(byte, open)
        } else {
            self.may_open(source, byte, open)
        }
    }

    fn empty_at_for(
        &mut self,
        source: &str,
        byte: usize,
        kind: crate::document::DocumentKind,
    ) -> bool {
        if kind == crate::document::DocumentKind::Tex {
            self.1.as_ref().unwrap().empty_at(byte)
        } else {
            self.empty_at(source, byte)
        }
    }

    fn may_step_over_for(
        &self,
        byte: usize,
        close: char,
        kind: crate::document::DocumentKind,
    ) -> bool {
        if kind == crate::document::DocumentKind::Tex {
            self.1.as_ref().unwrap().may_step_over(byte, close)
        } else {
            self.may_step_over(byte, close)
        }
    }
    fn raw_fence_at(&self, byte: usize) -> Option<Option<usize>> {
        let mut node = LinkedNode::new(self.0.root()).leaf_at(byte, Side::After)?;
        loop {
            if node.kind() == SyntaxKind::Raw {
                let mut delimiters = node
                    .children()
                    .filter(|child| child.kind() == SyntaxKind::RawDelim);
                if delimiters.next()?.offset() != byte {
                    return None;
                }
                return Some(delimiters.next().map(|closing| closing.offset()));
            }
            node = node.parent()?.clone();
        }
    }

    fn raw_delimiter_has_language(source: &str, byte: usize) -> bool {
        let ticks = source[byte..]
            .bytes()
            .take_while(|tick| *tick == b'`')
            .count();
        ticks >= 3
            && source[byte + ticks..]
                .chars()
                .next()
                .is_some_and(|character| {
                    character.is_ascii_alphanumeric() || "_-+".contains(character)
                })
    }

    pub(crate) fn newline(
        &mut self,
        source: &str,
        byte: usize,
        spaces: u8,
    ) -> Option<(String, usize)> {
        let line_start = source[..byte].rfind('\n').map_or(0, |at| at + 1);
        let line_ending = indentation::line_ending(source, byte, line_start);
        let prefix = &source[line_start..byte];
        let indent_len = prefix
            .bytes()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        let indent = &prefix[..indent_len];
        if source[..byte].ends_with('$') && source[byte..].starts_with('$') {
            self.prepare(source);
            if self.empty_at(source, byte) {
                let unit = if spaces == 0 {
                    "\t".into()
                } else {
                    " ".repeat(usize::from(spaces.min(16)))
                };
                let body = format!("{line_ending}{indent}{unit}");
                let advance = body.chars().count();
                return Some((format!("{body}{line_ending}{indent}"), advance));
            }
        }
        let last_tick = prefix.rfind('`')?;
        let fence_start = prefix[..=last_tick].trim_end_matches('`').len();
        let header = &prefix[fence_start..];
        let ticks = last_tick + 1 - fence_start;
        if ticks < 3
            || !header[ticks..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-+".contains(c))
        {
            return None;
        }
        self.prepare(source);
        let fence = &header[..ticks];
        let opening = line_start + fence_start;
        let closing = match self.raw_fence_at(opening) {
            Some(Some(at)) if !Self::raw_delimiter_has_language(source, at) => Some(at),
            Some(None) | Some(Some(_)) | None => {
                // A later tagged raw delimiter is another block opener, not
                // the closer for this unfinished opener.
                // Probe a complete block to validate its syntax context.
                self.0
                    .edit(byte..byte, &format!("{line_ending}x{line_ending}{fence}"));
                let closing_offset = byte + 2 * line_ending.len() + 1;
                if self.raw_fence_at(opening) != Some(Some(closing_offset)) {
                    return None;
                }
                None
            }
        };
        let body = format!("{line_ending}{indent}");
        let advance = body.chars().count();
        match closing {
            Some(at) if at == byte => Some((format!("{body}{line_ending}{indent}"), advance)),
            Some(_) => Some((body, advance)),
            None if byte == source.len()
                || source[byte..].starts_with(['\n', '\r', ')', ']', '}', ',']) =>
            {
                Some((format!("{body}{line_ending}{indent}{fence}"), advance))
            }
            None => {
                let trailing_line_ending = if byte < source.len() { line_ending } else { "" };
                Some((
                    format!("{body}{line_ending}{indent}{fence}{trailing_line_ending}"),
                    advance,
                ))
            }
        }
    }

    fn prepare(&mut self, source: &str) {
        self.0.replace(source);
    }

    fn literal_at(&self, byte: usize) -> bool {
        let Some(mut node) = LinkedNode::new(self.0.root()).leaf_at(byte, Side::Before) else {
            return false;
        };
        loop {
            let range = node.range();
            let inside = range.start < byte && byte < range.end;
            let literal = match node.kind() {
                SyntaxKind::LineComment => inside || byte == range.end,
                SyntaxKind::Str | SyntaxKind::Raw | SyntaxKind::BlockComment => {
                    inside || (byte == range.end && node.get().diagnosis().errors)
                }
                _ => false,
            };
            if literal {
                return true;
            }
            let Some(parent) = node.parent() else {
                return false;
            };
            node = parent.clone();
        }
    }

    fn may_open(&mut self, source: &str, byte: usize, open: char) -> bool {
        if self.literal_at(byte) || escaped_at(source, byte) {
            return false;
        }
        // Ordinary brackets also pair in prose; Typst's markup parser treats
        // prose parentheses and braces as text, unlike code/math delimiters.
        if "([{".contains(open) {
            return true;
        }
        let Some(close) = pairing::closer(open) else {
            return false;
        };
        // A payload makes empty labels/raw/emphasis parseable. Their syntax
        // decides whether punctuation is a delimiter (rather than multiply,
        // a math subscript, a comparison, or ordinary prose quotation).
        let probe = format!("{open}x{close}");
        self.0.edit(byte..byte, &probe);
        crate::delimiters::pair_at(&self.0, byte).is_some_and(|pair| {
            pair[0] == (byte..byte + open.len_utf8())
                && pair[1] == (byte + open.len_utf8() + 1..byte + probe.len())
        })
    }

    fn empty_at(&mut self, source: &str, byte: usize) -> bool {
        let before = source[..byte].chars().next_back();
        let after = source[byte..].chars().next();
        if !pairing::empty_pair(before, after) {
            return false;
        }
        if before.is_some_and(|open| "([{".contains(open))
            && !self.literal_at(byte)
            && !escaped_at(source, byte - 1)
        {
            return true;
        }
        // Empty raw fences, emphasis, and labels can be incomplete syntax.
        // Probe their context with content, just as when opening the pair.
        self.0.edit(byte..byte, "x");
        crate::delimiters::pair_at(&self.0, byte - 1)
            .is_some_and(|pair| pair == [byte - 1..byte, byte + 1..byte + 2])
    }

    fn may_step_over(&self, byte: usize, close: char) -> bool {
        if ")]}>$\"`*_".contains(close)
            && let Some(pair) = crate::delimiters::pair_at(&self.0, byte)
            && pair[1].start == byte
        {
            return true;
        }
        !self.literal_at(byte) && ")]}".contains(close)
    }
}

fn escaped_at(source: &str, byte: usize) -> bool {
    source[..byte]
        .chars()
        .rev()
        .take_while(|c| *c == '\\')
        .count()
        % 2
        == 1
}

fn closer_for(open: char, kind: crate::document::DocumentKind) -> Option<char> {
    if kind == crate::document::DocumentKind::Tex && !"([{$".contains(open) {
        None
    } else {
        pairing::closer(open)
    }
}

fn empty_pair_for(
    before: Option<char>,
    after: Option<char>,
    kind: crate::document::DocumentKind,
) -> bool {
    before
        .and_then(|open| closer_for(open, kind))
        .is_some_and(|close| Some(close) == after)
}

fn has_surround_typing(events: &[egui::Event]) -> bool {
    events.iter().any(|event| {
        matches!(event, egui::Event::Text(text)
        if text.chars().count() == 1 && text.chars().next().and_then(pairing::closer).is_some())
    })
}

pub(crate) struct PairingBuffer<'a> {
    source: &'a mut String,
    syntax: &'a mut PairSyntax,
    provisional: &'a mut provisional::State,
    owner: provisional::Owner,
    input_order: provisional::InputOrder,
    enabled: bool,
    indent_spaces: Option<u8>,
    typed_tab: bool,
    typed_enter: bool,
    document_kind: crate::document::DocumentKind,
    surround: bool,
    deleted_selection: Option<String>,
}

impl<'a> PairingBuffer<'a> {
    pub(crate) fn new(
        source: &'a mut String,
        syntax: &'a mut PairSyntax,
        provisional: &'a mut provisional::State,
        owner: provisional::Owner,
        enabled: bool,
        events: &[egui::Event],
    ) -> Self {
        // Composition and pasted batches are verbatim, even if a platform
        // also emits Text alongside them. Never pair transient IME preedit.
        let verbatim = events
            .iter()
            .any(|event| matches!(event, egui::Event::Paste(_) | egui::Event::Ime(_)));
        if !enabled || verbatim {
            provisional.cancel();
        }
        Self {
            source,
            syntax,
            provisional,
            owner,
            input_order: provisional::InputOrder::new(events),
            enabled: enabled && !verbatim,
            indent_spaces: None,
            typed_enter: !verbatim && events.iter().any(|event| matches!(event, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if modifiers.matches_logically(egui::Modifiers::NONE))),
            document_kind: crate::document::DocumentKind::Typst,
            surround: enabled && !verbatim && has_surround_typing(events),
            deleted_selection: None,
            typed_tab: events.iter().any(|event| matches!(event, egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } if !modifiers.shift)) && !verbatim,
        }
    }
}

impl Drop for PairingBuffer<'_> {
    fn drop(&mut self) {
        self.input_order.finish(self.provisional);
    }
}

impl PairingBuffer<'_> {
    pub(crate) fn with_surround(mut self, enabled: bool, events: &[egui::Event]) -> Self {
        self.surround = enabled
            && has_surround_typing(events)
            && !events
                .iter()
                .any(|event| matches!(event, egui::Event::Paste(_) | egui::Event::Ime(_)));
        self
    }

    pub(crate) fn with_indentation(mut self, spaces: u8) -> Self {
        self.indent_spaces = Some(spaces.min(16));
        self
    }

    pub(crate) fn with_document_kind(mut self, kind: crate::document::DocumentKind) -> Self {
        self.document_kind = kind;
        self
    }
}

impl PairingBuffer<'_> {
    fn insert_after_receipt(
        &mut self,
        cursor: &mut CCursor,
        text: &str,
        limit: usize,
        typed_input: bool,
    ) {
        if self.enabled && self.document_kind == crate::document::DocumentKind::Tex && typed_input {
            match self.provisional.continue_text(
                self.owner,
                self.source,
                cursor.index.0,
                text,
                limit,
            ) {
                provisional::TextAction::Advance(advance) => {
                    cursor.index += advance;
                    return;
                }
                provisional::TextAction::Complete { advance, tail } => {
                    cursor.index += advance;
                    self.insert_after_receipt(cursor, &text[tail..], limit, typed_input);
                    return;
                }
                provisional::TextAction::Restore { insertion, advance } => {
                    self.source.insert_text(&insertion, cursor.index);
                    cursor.index += advance;
                    return;
                }
                provisional::TextAction::AtLimit => return,
                provisional::TextAction::Ordinary => {}
            }
        } else {
            self.provisional.cancel();
        }
        if self.enabled
            && self.document_kind == crate::document::DocumentKind::Tex
            && (matches!(text, "[" | "(" | "]" | ")")
                || text.starts_with('\\')
                || (self.typed_enter && text == "\n"))
        {
            let byte = self.source.byte_index_from_char_index(cursor.index).0;
            let next = &self.source[byte..];
            if (text.starts_with('\\') || matches!(text, "]" | ")"))
                && (next.starts_with('\\')
                    || (self.source[..byte].ends_with('\\') && next.starts_with([']', ')'])))
            {
                self.syntax.prepare_for(self.source, self.document_kind);
                if let Some(advance) = self.syntax.1.as_ref().unwrap().command_step(byte, text) {
                    if typed_input
                        && text == "\\"
                        && let Some(span) = self.syntax.1.as_ref().unwrap().command_closer(byte)
                        && span.start == byte
                    {
                        self.provisional
                            .begin(self.owner, self.source, span, cursor.index.0);
                    }
                    cursor.index += advance;
                    return;
                }
            }
            if matches!(text, "[" | "(") && self.source[..byte].ends_with('\\') {
                self.syntax.prepare_for(self.source, self.document_kind);
                if let Some(close) = self
                    .syntax
                    .1
                    .as_ref()
                    .unwrap()
                    .command_opener(byte, text.chars().next().unwrap())
                    && (self.deleted_selection.is_some()
                        || matches!(
                            pairing::typed_action(
                                text.chars().next().unwrap(),
                                next.chars().next(),
                                true,
                                false
                            ),
                            TypedAction::InsertPair(_)
                        ))
                {
                    let selected = self.deleted_selection.as_deref().unwrap_or("");
                    let inserted = format!("{text}{selected}\\{close}");
                    if limit.saturating_sub(self.source.chars().count()) >= inserted.chars().count()
                    {
                        let advance = if self.deleted_selection.is_some() {
                            inserted.chars().count()
                        } else {
                            1
                        };
                        self.deleted_selection = None;
                        self.source.insert_text(&inserted, cursor.index);
                        cursor.index += advance;
                        return;
                    }
                }
            }
            if self.typed_enter && text == "\n" {
                let mut headers = indentation::environment_headers(self.source, byte).peekable();
                if headers.peek().is_some() {
                    self.syntax.prepare_for(self.source, self.document_kind);
                }
                let header = headers.find(|header| {
                    self.syntax
                        .1
                        .as_ref()
                        .unwrap()
                        .may_complete_environment(header.start)
                });
                drop(headers);
                if let Some(header) = header {
                    let edit = indentation::complete_environment(
                        self.source,
                        byte,
                        &header,
                        self.indent_spaces.unwrap_or(2),
                    );
                    if self
                        .source
                        .chars()
                        .count()
                        .saturating_sub(edit.trim_after)
                        .saturating_add(edit.text.chars().count())
                        <= limit
                    {
                        self.source
                            .delete_char_range(cursor.index..cursor.index + edit.trim_after);
                        self.source.insert_text(&edit.text, cursor.index);
                        cursor.index += edit.advance;
                        return;
                    }
                }
            }
        }
        if let Some(selected) = self.deleted_selection.take()
            && text.chars().count() == 1
            && let Some(close) = text.chars().next().and_then(pairing::closer)
        {
            self.source
                .insert_text_at(cursor, &format!("{text}{selected}{close}"), limit);
            return;
        }

        if self.typed_tab
            && text == "\t"
            && let Some(spaces) = self.indent_spaces
            && spaces > 0
        {
            self.source
                .insert_text_at(cursor, &" ".repeat(spaces as usize), limit);
            return;
        }
        if self.enabled && self.typed_enter && self.document_kind.is_typst() && text == "\n" {
            let byte = self.source.byte_index_from_char_index(cursor.index).0;
            if let Some((inserted, advance)) =
                self.syntax
                    .newline(self.source, byte, self.indent_spaces.unwrap_or(2))
                && limit.saturating_sub(self.source.chars().count()) >= inserted.chars().count()
            {
                self.source.insert_text(&inserted, cursor.index);
                cursor.index += advance;
                return;
            }
        }
        if self.typed_enter
            && text == "\n"
            && matches!(
                self.document_kind,
                crate::document::DocumentKind::Typst | crate::document::DocumentKind::Tex
            )
            && let Some(spaces) = self.indent_spaces
        {
            let byte = self.source.byte_index_from_char_index(cursor.index).0;
            let edit = indentation::newline(self.source, byte, self.document_kind, spaces);
            let fits = limit == usize::MAX
                || self
                    .source
                    .chars()
                    .count()
                    .saturating_sub(edit.trim_after)
                    .saturating_add(edit.text.chars().count())
                    <= limit;
            if fits {
                if edit.trim_after > 0 {
                    self.source
                        .delete_char_range(cursor.index..cursor.index + edit.trim_after);
                }
                self.source.insert_text(&edit.text, cursor.index);
                cursor.index += edit.advance;
                return;
            }
        }
        if self.enabled && self.document_kind.is_typst() && text == " " {
            let byte = self.source.byte_index_from_char_index(cursor.index).0;
            if self.source[..byte].ends_with('$') && self.source[byte..].starts_with('$') {
                self.syntax.prepare(self.source);
                if self.syntax.empty_at(self.source, byte)
                    && limit.saturating_sub(self.source.chars().count()) >= 2
                {
                    self.source.insert_text("  ", cursor.index);
                    cursor.index += 1;
                    return;
                }
            }
        }
        // Completing a triple fence is not opening a fourth,
        // paired single backtick. Its matching fence is inserted on Enter.
        if self.enabled && self.document_kind.is_typst() && text == "`" {
            let byte = self.source.byte_index_from_char_index(cursor.index).0;
            if self.source[..byte].ends_with("``") && !self.source[..byte].ends_with("```") {
                if self.source[byte..].starts_with('`') {
                    // The first backtick created an empty single-backtick
                    // pair. Treat the third typed backtick as the end of the
                    // opening fence by stepping over that generated closer;
                    // inserting another one would leave four ticks and make
                    // Enter fail to recognize the fence.
                    cursor.index += 1;
                } else {
                    self.source.insert_text_at(cursor, text, limit);
                }
                return;
            }
        }
        let mut chars = text.chars();
        let typed = chars.next();
        if self.enabled
            && chars.next().is_none()
            && let Some(typed) = typed
            && "()[]{}<>$\"`*_".contains(typed)
        {
            let byte = self.source.byte_index_from_char_index(cursor.index).0;
            let next = self.source[byte..].chars().next();
            self.syntax.prepare_for(self.source, self.document_kind);
            if self.document_kind == crate::document::DocumentKind::Tex
                && typed == '$'
                && self.syntax.1.as_ref().unwrap().expand_empty_math(byte)
                && limit.saturating_sub(self.source.chars().count()) >= 2
            {
                self.source.insert_text("$$", cursor.index);
                cursor.index += 1;
                return;
            }
            let step = next == Some(typed)
                && (self
                    .syntax
                    .may_step_over_for(byte, typed, self.document_kind)
                    || self
                        .syntax
                        .empty_at_for(self.source, byte, self.document_kind));
            self.syntax.prepare_for(self.source, self.document_kind);
            let open = !step
                && self
                    .syntax
                    .may_open_for(self.source, byte, typed, self.document_kind);
            match pairing::typed_action(typed, next, open, step) {
                TypedAction::StepOver => {
                    cursor.index += 1;
                    return;
                }
                TypedAction::InsertPair(close)
                    if limit.saturating_sub(self.source.chars().count()) >= 2 =>
                {
                    self.source
                        .insert_text(&format!("{typed}{close}"), cursor.index);
                    cursor.index += 1;
                    return;
                }
                _ => {}
            }
        }
        self.source.insert_text_at(cursor, text, limit);
    }
}

impl TextBuffer for PairingBuffer<'_> {
    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        self.source
    }
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<PairingBuffer<'static>>()
    }
    fn insert_text(&mut self, text: &str, index: CharIndex) -> usize {
        self.provisional.cancel();
        self.source.insert_text(text, index)
    }
    fn delete_char_range(&mut self, range: Range<CharIndex>) {
        if !range.is_empty() {
            self.provisional.cancel();
        }
        self.source.delete_char_range(range);
    }
    fn delete_selected(&mut self, selection: &egui::text::CCursorRange) -> CCursor {
        let range = selection.as_sorted_char_range();
        if !range.is_empty() {
            self.provisional.cancel();
        }
        self.deleted_selection =
            (self.surround && !range.is_empty()).then(|| self.source.char_range(range).to_owned());
        self.source.delete_selected(selection)
    }
    fn insert_text_at(&mut self, cursor: &mut CCursor, text: &str, limit: usize) {
        let typed_input = self.input_order.before_text(self.provisional, text);
        self.insert_after_receipt(cursor, text, limit, typed_input);
    }
    fn decrease_indentation(&mut self, cursor: &mut CCursor) {
        self.provisional.cancel();
        let byte = self.source.byte_index_from_char_index(cursor.index).0;
        let start_byte = self.source[..byte].rfind('\n').map_or(0, |at| at + 1);
        let start = self.source[..start_byte].chars().count();
        let width = self.indent_spaces.unwrap_or(4).max(1) as usize;
        let count = if self.source[start_byte..].starts_with('\t') {
            1
        } else {
            self.source[start_byte..]
                .chars()
                .take(width)
                .take_while(|c| *c == ' ')
                .count()
        };
        self.source
            .delete_char_range(CharIndex(start)..CharIndex(start + count));
        cursor.index = CharIndex(cursor.index.0.saturating_sub(count).max(start));
    }
    fn delete_previous_char(&mut self, cursor: CCursor) -> CCursor {
        let typed_backspace = self.input_order.before_backspace(self.provisional);
        if typed_backspace
            && self.enabled
            && self.document_kind == crate::document::DocumentKind::Tex
            && let Some(caret) = self
                .provisional
                .rewind(self.owner, self.source, cursor.index.0)
        {
            return CCursor::new(caret);
        }
        self.provisional.cancel();
        if self.enabled && cursor.index.0 > 0 {
            let byte = self.source.byte_index_from_char_index(cursor.index).0;
            if self.document_kind == crate::document::DocumentKind::Tex
                && (self.source[..byte].ends_with("\\[") || self.source[..byte].ends_with("\\("))
                && (self.source[byte..].starts_with("\\]")
                    || self.source[byte..].starts_with("\\)"))
            {
                self.syntax.prepare_for(self.source, self.document_kind);
                if self.syntax.1.as_ref().unwrap().empty_command(byte) {
                    self.source
                        .delete_char_range(cursor.index - 2..cursor.index + 2);
                    return cursor - 2;
                }
            }
            let before = self.source[..byte].chars().next_back();
            let after = self.source[byte..].chars().next();
            if empty_pair_for(before, after, self.document_kind) {
                self.syntax.prepare_for(self.source, self.document_kind);
                if self
                    .syntax
                    .empty_at_for(self.source, byte, self.document_kind)
                {
                    self.source
                        .delete_char_range(cursor.index - 1..cursor.index + 1);
                    return cursor - 1;
                }
            }
        }
        self.source.delete_previous_char(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::text::CCursorRange;
    use tiptoptyp_core::document::{DocumentKind, DocumentSession, WindowSessionId};

    fn test_owner() -> provisional::Owner {
        provisional::Owner {
            document: tiptoptyp_core::document::DocumentKey::new(WindowSessionId::new(1), 0, 0),
            tab: 0,
            editor: egui::Id::new("paired-editor"),
        }
    }

    fn tex_typed(source: &str, caret: usize, text: &str) -> (String, usize) {
        let mut source = source.to_owned();
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut cursor = CCursor::new(caret);
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &[egui::Event::Text(text.into())],
        )
        .with_document_kind(DocumentKind::Tex)
        .insert_text_at(&mut cursor, text, usize::MAX);
        assert!(
            syntax.0.text().is_empty(),
            "TeX must not be parsed as Typst"
        );
        (source, cursor.index.0)
    }

    #[test]
    fn tex_command_math_openers_insert_their_matching_commands() {
        assert_eq!(tex_typed("\\", 1, "["), ("\\[\\]".into(), 2));
        assert_eq!(tex_typed("文🦀\\", 3, "("), ("文🦀\\(\\)".into(), 4));
    }

    #[test]
    fn tex_command_closers_overtype_and_empty_commands_delete_atomically() {
        for (source, caret, text, expected) in [
            ("\\[x\\]", 3, "\\", 4),
            ("\\[x\\]", 4, "]", 5),
            ("\\[x\\]", 3, "]", 5),
            ("\\(x\\)", 3, "\\)", 5),
            ("\\(x\\)", 3, ")", 5),
        ] {
            assert_eq!(tex_typed(source, caret, text), (source.into(), expected));
        }
        for source in ["\\[\\]", "\\(\\)"] {
            let mut source = format!("文🦀{source}");
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let cursor = PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &[],
            )
            .with_document_kind(DocumentKind::Tex)
            .delete_previous_char(CCursor::new(4));
            assert_eq!(source, "文🦀");
            assert_eq!(cursor.index.0, 2);
        }
    }

    #[test]
    fn native_tex_command_pairing_overtype_and_backspace_keep_atomic_history() {
        for (open, close) in [("[", "]"), ("(", ")")] {
            let ctx = egui::Context::default();
            let mut syntax = PairSyntax::default();
            let mut document =
                DocumentSession::new(WindowSessionId::new(1), "\\", DocumentKind::Tex);
            let body = frame(
                &ctx,
                &mut document,
                &mut syntax,
                vec![egui::Event::Text(open.into())],
                true,
                CCursorRange::one(CCursor::new(1)),
            );
            let paired = format!("\\{open}\\{close}");
            assert_eq!(document.source(), &paired);
            let paired_key = document.key();
            let after = frame(
                &ctx,
                &mut document,
                &mut syntax,
                vec![
                    egui::Event::Text("\\".into()),
                    egui::Event::Text(close.into()),
                ],
                true,
                body,
            );
            assert_eq!(
                document.key(),
                paired_key,
                "overtype must not create a document revision"
            );
            assert_eq!(after.primary.index.0, 4);
            let deleted = frame(
                &ctx,
                &mut document,
                &mut syntax,
                vec![key(egui::Key::Backspace)],
                true,
                body,
            );
            assert_eq!(document.source(), "");
            let undo = document.history_step(false, deleted).unwrap();
            assert_eq!(document.source(), &paired);
            assert_eq!(undo.cursor, body);
            document.history_step(true, undo.cursor).unwrap();
            assert_eq!(document.source(), "");
        }
    }

    #[test]
    fn tex_command_pairing_respects_literals_escapes_settings_and_limits() {
        for prefix in [
            r"% comment \",
            r"\verb|\",
            r"\url{\",
            "\\begin{verbatim}\n\\",
            r"$x\",
        ] {
            assert_eq!(
                tex_typed(prefix, prefix.chars().count(), "["),
                (format!("{prefix}["), prefix.chars().count() + 1)
            );
        }
        assert_eq!(tex_typed("\\\\", 2, "["), ("\\\\[]".into(), 3));
        for (event, enabled) in [
            (egui::Event::Text("[".into()), false),
            (egui::Event::Paste("[".into()), true),
            (egui::Event::Ime(egui::ImeEvent::Commit("[".into())), true),
        ] {
            let mut source = "\\".into();
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let mut cursor = CCursor::new(1);
            PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                enabled,
                &[event],
            )
            .with_document_kind(DocumentKind::Tex)
            .insert_text_at(&mut cursor, "[", usize::MAX);
            assert_eq!(source, "\\[");
        }
        let mut source = "\\".into();
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut cursor = CCursor::new(1);
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &[egui::Event::Text("[".into())],
        )
        .with_document_kind(DocumentKind::Tex)
        .insert_text_at(&mut cursor, "[", 2);
        assert_eq!(source, "\\[");
    }

    #[test]
    fn tex_pairs_groups_optional_arguments_and_math_without_markup_rules() {
        for (source, caret, text, expected, after) in [
            ("", 0, "{", "{}", 1),
            ("\\section", 8, "{", "\\section{}", 9),
            ("\\includegraphics", 16, "[", "\\includegraphics[]", 17),
            ("文🦀", 2, "(", "文🦀()", 3),
            ("", 0, "$", "$$", 1),
            ("$$", 1, "$", "$$$$", 2),
            ("$x$", 2, "$", "$x$", 3),
            ("$$x$$", 3, "$", "$$x$$", 4),
            ("$$x$$", 4, "$", "$$x$$", 5),
            ("$x", 2, "$", "$x$", 3),
            ("$$x", 3, "$", "$$x$", 4),
            ("$$x$", 4, "$", "$$x$$", 5),
            ("{}", 1, "}", "{}", 2),
            ("\\url{}", 5, "}", "\\url{}", 6),
            ("[]", 1, "]", "[]", 2),
            ("()", 1, ")", "()", 2),
        ] {
            assert_eq!(
                tex_typed(source, caret, text),
                (expected.into(), after),
                "{source:?} + {text:?}"
            );
        }
        for text in ["*", "_", "\"", "`", "<", ">"] {
            assert_eq!(tex_typed("", 0, text), (text.into(), 1));
        }
    }

    #[test]
    fn tex_comments_escapes_and_literal_payloads_keep_typed_punctuation_verbatim() {
        for source in [
            "% comment",
            "\\verb|literal",
            "\\verb|literal|% comment",
            "\\lstinline[language=TeX]|literal",
            "\\mintinline{tex}|literal",
            "\\begin{verbatim}\nliteral",
            "\\begin{Verbatim*}\nliteral",
            "\\begin{BVerbatim}\nliteral",
            "\\begin{comment}\nliteral",
        ] {
            for text in ["(", "[", "{", "$", "_"] {
                assert_eq!(
                    tex_typed(source, source.chars().count(), text),
                    (format!("{source}{text}"), source.chars().count() + 1),
                    "{source:?} + {text}"
                );
            }
        }
        for text in ["{", "$", "_"] {
            assert_eq!(tex_typed("\\", 1, text), (format!("\\{text}"), 2));
        }
        for source in ["\\verb|literal|", "\\begin{verbatim}\nx\n\\end{verbatim}"] {
            assert_eq!(
                tex_typed(source, source.chars().count(), "("),
                (format!("{source}()"), source.chars().count() + 1)
            );
        }
    }

    #[test]
    fn tex_empty_pair_deletion_and_cached_index_do_not_touch_typst_syntax() {
        for (source, caret, expected, after) in [
            ("{}", 1, "", 0),
            ("[]", 1, "", 0),
            ("()", 1, "", 0),
            ("$$", 1, "", 0),
            ("$$$$", 2, "$$", 1),
            ("\\url{}", 5, "\\url", 4),
            ("% ()", 3, "% )", 2),
            ("\\verb|()|", 7, "\\verb|)|", 6),
        ] {
            let mut source = source.into();
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let cursor = PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &[],
            )
            .with_document_kind(DocumentKind::Tex)
            .delete_previous_char(CCursor::new(caret));
            assert_eq!((source, cursor.index.0), (expected.into(), after));
            assert!(syntax.0.text().is_empty());
        }
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut source = "{}".into();
        let mut cursor = CCursor::new(1);
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &[],
        )
        .with_document_kind(DocumentKind::Tex)
        .insert_text_at(&mut cursor, "}", usize::MAX);
        syntax.prepare_for(&source, DocumentKind::Tex);
        syntax.prepare_for(&source, DocumentKind::Tex);
        assert_eq!(syntax.1.as_ref().unwrap().builds, 1);
        let mut source = String::new();
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &[],
        )
        .with_document_kind(DocumentKind::Tex)
        .insert_text_at(&mut CCursor::new(0), "word", usize::MAX);
        assert!(syntax.1.is_none());
    }

    #[test]
    fn tex_explicit_selection_keeps_shared_wrapping_but_paste_and_ime_stay_literal() {
        for (text, event, expected) in [
            ("_", egui::Event::Text("_".into()), "_selected_"),
            ("*", egui::Event::Text("*".into()), "*selected*"),
            ("\"", egui::Event::Text("\"".into()), "\"selected\""),
            ("(", egui::Event::Paste("(".into()), "("),
            (
                "(",
                egui::Event::Ime(egui::ImeEvent::Commit("(".into())),
                "(",
            ),
        ] {
            let mut source = "selected".into();
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let mut buffer = PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &[event],
            )
            .with_document_kind(DocumentKind::Tex);
            let mut cursor =
                buffer.delete_selected(&CCursorRange::two(CCursor::new(0), CCursor::new(8)));
            buffer.insert_text_at(&mut cursor, text, usize::MAX);
            drop(buffer);
            assert_eq!(source, expected);
            assert_eq!(cursor.index.0, expected.chars().count());
        }
    }

    fn typed(source: &str, caret: usize, text: &str) -> (String, usize) {
        let mut source = source.to_owned();
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut cursor = CCursor::new(caret);
        let events = if text == "\n" {
            vec![key(egui::Key::Enter)]
        } else {
            vec![]
        };
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &events,
        )
        .insert_text_at(&mut cursor, text, usize::MAX);
        (source, cursor.index.0)
    }

    #[test]
    fn types_nested_pairs_and_symmetric_delimiters_in_their_syntax_context() {
        for (source, caret, input, expected) in [
            ("", 0, "(", "()"),
            ("中文🦀", 3, "[", "中文🦀[]"),
            ("#let x = ", 9, "{", "#let x = {}"),
            ("#let x = ", 9, "\"", "#let x = \"\""),
            ("", 0, "$", "$$"),
            ("", 0, "<", "<>"),
            ("", 0, "`", "``"),
            ("", 0, "*", "**"),
            ("", 0, "_", "__"),
            ("#let x = ()", 10, "[", "#let x = ([])"),
        ] {
            assert_eq!(
                typed(source, caret, input),
                (expected.into(), caret + 1),
                "{source:?} + {input}"
            );
            assert_eq!(
                typed(
                    expected,
                    caret + 1,
                    &pairing::closer(input.chars().next().unwrap())
                        .unwrap()
                        .to_string()
                ),
                (expected.into(), caret + 2),
                "step over {input}"
            );
            let mut expected = expected.to_owned();
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let cursor = PairingBuffer::new(
                &mut expected,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &[],
            )
            .delete_previous_char(CCursor::new(caret + 1));
            assert_eq!(
                (expected, cursor.index.0),
                (source.into(), caret),
                "backspace {input}"
            );
        }
    }

    #[test]
    fn enter_expands_fences_and_empty_math_with_the_caret_inside() {
        for (before, expected) in [
            ("```tex|", "```tex\n|\n```"),
            ("```|", "```\n|\n```"),
            ("  ```typ|", "  ```typ\n  |\n  ```"),
            ("````latex|", "````latex\n|\n````"),
            ("```|\nexisting", "```\n|\n```\nexisting"),
            ("```tex|\nexisting", "```tex\n|\n```\nexisting"),
            ("before\n```|\nafter", "before\n```\n|\n```\nafter"),
            ("before\n```tex|\nafter", "before\n```tex\n|\n```\nafter"),
            (
                "before\n  ```tex|\nafter",
                "before\n  ```tex\n  |\n  ```\nafter",
            ),
            (
                "before\r\n```tex|\r\nafter",
                "before\r\n```tex\r\n|\r\n```\r\nafter",
            ),
            ("```tex|body", "```tex\n|\n```\nbody"),
            ("text ```tex|body", "text ```tex\n|\n```\nbody"),
            (
                "```tex|\n```tex\n\\test=1\n```",
                "```tex\n|\n```\n```tex\n\\test=1\n```",
            ),
            ("```tex|```", "```tex\n|\n```"),
            ("```tex|\n```", "```tex\n|\n```"),
            ("#mi(```tex|)", "#mi(```tex\n|\n```)"),
            ("text ```tex|", "text ```tex\n|\n```"),
            ("$|$", "$\n  |\n$"),
            ("  $|$", "  $\n    |\n  $"),
            ("文 $|$", "文 $\n  |\n$"),
        ] {
            let cursor = before[..before.find('|').unwrap()].chars().count();
            let expected_cursor = expected[..expected.find('|').unwrap()].chars().count();
            assert_eq!(
                typed(&before.replace('|', ""), cursor, "\n"),
                (expected.replace('|', ""), expected_cursor),
                "{before}"
            );
        }
    }

    fn typed_chars(source: &str, caret: usize, text: &str) -> (String, usize) {
        let mut source = source.to_owned();
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut cursor = CCursor::new(caret);
        for typed in text.chars() {
            PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &[],
            )
            .insert_text_at(&mut cursor, &typed.to_string(), usize::MAX);
        }
        (source, cursor.index.0)
    }

    #[test]
    fn typing_three_backticks_turns_the_generated_closer_into_the_fence() {
        for opening in ["```", "```tex"] {
            let (source, caret) = typed_chars("", 0, opening);
            assert_eq!(source, opening);
            assert_eq!(caret, opening.chars().count());
            assert_eq!(
                typed(&source, caret, "\n"),
                (format!("{opening}\n\n```"), opening.chars().count() + 1)
            );
        }
    }

    #[test]
    fn space_inside_an_empty_math_pair_is_inserted_on_both_sides_of_the_caret() {
        for (source, caret, expected, expected_caret) in [
            ("$$", 1, "$  $", 2),
            ("  $$", 3, "  $  $", 4),
            ("$x$", 1, "$ x$", 2),
        ] {
            assert_eq!(
                typed(source, caret, " "),
                (expected.into(), expected_caret),
                "{source:?} at {caret}"
            );
        }

        let (source, caret) = typed_chars("", 0, "$");
        assert_eq!(typed(&source, caret, " "), ("$  $".into(), 2));
    }

    #[test]
    fn enter_does_not_pair_closing_fences_comments_strings_or_math_contents() {
        for before in [
            "```tex\nx\n```|",
            "// ```tex|",
            "/*\n```tex|\n*/",
            "#let x = \"$|$\"",
            "```tex\n$|$\n```",
            "$x|$",
            "\\$|$",
            "````\n```tex|\n````",
        ] {
            let byte = before.find('|').unwrap();
            let cursor = before[..byte].chars().count();
            assert_eq!(
                typed(&before.replace('|', ""), cursor, "\n"),
                (before.replace('|', "\n"), cursor + 1),
                "{before}"
            );
        }
    }

    #[test]
    fn enter_obeys_disabled_pairing_verbatim_input_and_character_limit() {
        for (enabled, events, limit) in [
            (false, vec![], usize::MAX),
            (true, vec![egui::Event::Paste("\n".into())], usize::MAX),
            (
                true,
                vec![egui::Event::Ime(egui::ImeEvent::Commit("\n".into()))],
                usize::MAX,
            ),
            (true, vec![], 3),
        ] {
            let mut source = "$$".to_owned();
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let mut cursor = CCursor::new(1);
            PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                enabled,
                &events,
            )
            .insert_text_at(&mut cursor, "\n", limit);
            assert_eq!(source, "$\n$");
            assert_eq!(cursor.index.0, 2);
        }
    }

    #[test]
    fn newline_prediction_obeys_limits_and_math_body_uses_the_configured_unit() {
        for (spaces, expected, caret) in [(0, "$\n\t\n$", 3), (4, "$\n    \n$", 6)] {
            let mut source = "$$".to_owned();
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let mut cursor = CCursor::new(1);
            PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &[key(egui::Key::Enter)],
            )
            .with_indentation(spaces)
            .insert_text_at(&mut cursor, "\n", usize::MAX);
            assert_eq!(source, expected);
            assert_eq!(cursor.index.0, caret);
        }
        let mut source = "#let f() = {  }".to_owned();
        let before = source.clone();
        let limit = source.chars().count() + 1;
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut cursor = CCursor::new(before.find('{').unwrap() + 1);
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            false,
            &[key(egui::Key::Enter)],
        )
        .with_indentation(2)
        .insert_text_at(&mut cursor, "\n", limit);
        let mut expected = before;
        expected.insert(expected.find('{').unwrap() + 1, '\n');
        assert_eq!(
            source, expected,
            "failed prediction must not trim original whitespace"
        );
    }

    #[test]
    fn ordinary_enter_does_not_parse() {
        let mut source = "ordinary text".to_owned();
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut cursor = CCursor::new(source.chars().count());
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &[key(egui::Key::Enter)],
        )
        .with_indentation(2)
        .insert_text_at(&mut cursor, "\n", usize::MAX);
        assert_eq!(source, "ordinary text\n");
        assert!(syntax.0.text().is_empty());
    }

    #[test]
    fn literal_text_paste_words_and_operators_are_not_paired() {
        for (source, caret, input) in [
            ("// comment ", 11, "("),
            ("/* comment */", 5, "["),
            ("#let x = \"text\"", 12, "["),
            ("`text`", 3, "{"),
            ("\\", 1, "["),
            ("$ x ", 4, "*"),
            ("$ x", 3, "_"),
            ("$ x ", 4, "<"),
            ("word", 0, "("),
            ("", 0, "\""),
            ("", 0, "([])"),
        ] {
            let byte = source
                .char_indices()
                .nth(caret)
                .map_or(source.len(), |(byte, _)| byte);
            let mut expected = source.to_owned();
            expected.insert_str(byte, input);
            assert_eq!(
                typed(source, caret, input),
                (expected, caret + input.chars().count()),
                "{source:?}"
            );
        }
        for event in [
            egui::Event::Paste("(".into()),
            egui::Event::Ime(egui::ImeEvent::Commit("(".into())),
        ] {
            let mut source = String::new();
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &[event],
            )
            .insert_text_at(&mut CCursor::new(0), "(", usize::MAX);
            assert_eq!(source, "(");
        }
    }

    #[test]
    fn steps_over_closers_without_eating_literal_characters() {
        for (source, caret, input) in [
            ("()", 1, ")"),
            ("#let x = \"\"", 10, "\""),
            ("$x$", 2, "$"),
            ("<x>", 2, ">"),
            ("`x`", 2, "`"),
        ] {
            assert_eq!(typed(source, caret, input), (source.into(), caret + 1));
        }
        assert_eq!(
            typed("#let x = \"()\"", 11, ")"),
            ("#let x = \"())\"".into(), 12)
        );
    }

    #[test]
    fn non_delimiter_typing_does_not_parse_and_respects_character_limits() {
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut source = String::new();
        let mut cursor = CCursor::new(0);
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &[],
        )
        .insert_text_at(&mut cursor, "文", usize::MAX);
        assert!(syntax.0.text().is_empty());
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &[],
        )
        .insert_text_at(&mut cursor, "(", 2);
        assert_eq!(source, "文(");
        assert_eq!(cursor.index.0, 2);
        PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &[],
        )
        .insert_text_at(&mut cursor, "[", 2);
        assert_eq!(source, "文(");
        assert_eq!(
            typed("/* closed */", 12, "("),
            ("/* closed */()".into(), 13)
        );
        assert_eq!(typed("\\[", 2, "("), ("\\[()".into(), 3));
    }

    fn frame(
        ctx: &egui::Context,
        document: &mut DocumentSession<CCursorRange>,
        syntax: &mut PairSyntax,
        events: Vec<egui::Event>,
        enabled: bool,
        selection: CCursorRange,
    ) -> CCursorRange {
        let id = egui::Id::new("paired-editor");
        let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(selection));
        state.store(ctx, id);
        ctx.memory_mut(|memory| memory.request_focus(id));
        let mut result = selection;
        let input = egui::RawInput {
            events: events.clone(),
            ..Default::default()
        };
        let kind = document.kind();
        let owner = provisional::Owner {
            document: document.key(),
            ..test_owner()
        };
        let shared = ctx.data_mut(|data| {
            let state = data
                .get_temp::<std::sync::Arc<std::sync::Mutex<provisional::State>>>(
                    id.with("input-state"),
                )
                .unwrap_or_default();
            data.insert_temp(id.with("input-state"), state.clone());
            state
        });
        let mut input_state = shared.lock().unwrap();
        ctx.run_ui(input, |ui| {
            input_state.begin_frame(ui.ctx().cumulative_frame_nr());
            input_state.reconcile(
                owner,
                document.source(),
                selection.is_empty().then_some(selection.primary.index.0),
                enabled && kind == DocumentKind::Tex,
            );
            document.edit(selection, |source| {
                let mut buffer =
                    PairingBuffer::new(source, syntax, &mut input_state, owner, enabled, &events)
                        .with_indentation(2)
                        .with_document_kind(kind);
                let output = egui::TextEdit::multiline(&mut buffer)
                    .code_editor()
                    .id(id)
                    .show(ui);
                result = output.state.cursor.char_range().unwrap();
            });
            input_state.finish(
                owner,
                document.key(),
                document.source(),
                result.is_empty().then_some(result.primary.index.0),
                enabled && kind == DocumentKind::Tex,
                ui.ctx().cumulative_frame_nr(),
            );
            input_state.end_frame(ui.ctx().cumulative_frame_nr());
        })
        .drop_without_applying_deltas();
        result
    }
    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn native_tex_typed_prefix_recovers_the_closer_across_and_within_frames() {
        for (source, at, content) in [
            (r"\(\)", 2, r"\(\a\)"),
            (r"\[\]", 2, r"\[\a\]"),
            (r"\begin{env}x\end{env}", 12, r"\begin{env}x\a\end{env}"),
        ] {
            for same_frame in [false, true] {
                let ctx = egui::Context::default();
                let mut syntax = PairSyntax::default();
                let mut doc =
                    DocumentSession::new(WindowSessionId::new(1), source, DocumentKind::Tex);
                let mut cursor = CCursorRange::one(CCursor::new(at));
                let before = doc.key();
                if same_frame {
                    cursor = frame(
                        &ctx,
                        &mut doc,
                        &mut syntax,
                        vec![
                            egui::Event::Text("\\".into()),
                            egui::Event::Text("a".into()),
                        ],
                        true,
                        cursor,
                    );
                } else {
                    cursor = frame(
                        &ctx,
                        &mut doc,
                        &mut syntax,
                        vec![egui::Event::Text("\\".into())],
                        true,
                        cursor,
                    );
                    assert_eq!(doc.key(), before);
                    assert_eq!(doc.source(), source);
                    cursor = frame(
                        &ctx,
                        &mut doc,
                        &mut syntax,
                        vec![egui::Event::Text("a".into())],
                        true,
                        cursor,
                    );
                }
                assert_eq!(doc.source(), content, "same_frame={same_frame}, {source}");
                assert_eq!(cursor.primary.index.0, at + 2);
                let undo = doc.history_step(false, cursor).unwrap();
                assert_eq!(doc.source(), source);
                doc.history_step(true, undo.cursor).unwrap();
                assert_eq!(doc.source(), content);
            }
        }
    }

    #[test]
    fn native_tex_manual_arrival_and_navigation_barriers_insert_literally() {
        for provisional in [false, true] {
            let ctx = egui::Context::default();
            let mut syntax = PairSyntax::default();
            let mut doc = DocumentSession::new(WindowSessionId::new(1), r"\(\)", DocumentKind::Tex);
            let cursor = CCursorRange::one(CCursor::new(if provisional { 2 } else { 3 }));
            let mut events = vec![];
            if provisional {
                events.push(egui::Event::Text("\\".into()));
            }
            events.extend([
                key(egui::Key::ArrowLeft),
                key(egui::Key::ArrowRight),
                egui::Event::Text("a".into()),
            ]);
            frame(&ctx, &mut doc, &mut syntax, events, true, cursor);
            assert_eq!(doc.source(), r"\(\a)");
        }
    }

    #[test]
    fn native_tex_provisional_backspace_and_complete_closer_do_not_change_source() {
        for suffix in [
            vec![key(egui::Key::Backspace)],
            vec![egui::Event::Text(")".into())],
        ] {
            let ctx = egui::Context::default();
            let mut syntax = PairSyntax::default();
            let mut doc = DocumentSession::new(WindowSessionId::new(1), r"\(\)", DocumentKind::Tex);
            let before = doc.key();
            let cursor = frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![egui::Event::Text("\\".into())],
                true,
                CCursorRange::one(CCursor::new(2)),
            );
            let backspace = matches!(suffix[0], egui::Event::Key { .. });
            let cursor = frame(&ctx, &mut doc, &mut syntax, suffix, true, cursor);
            assert_eq!(doc.key(), before);
            assert_eq!(doc.source(), r"\(\)");
            assert_eq!(cursor.primary.index.0, if backspace { 2 } else { 4 });
            assert_eq!(doc.history_availability(), (false, false));
        }
    }

    #[test]
    fn native_tex_paste_and_ime_retire_the_prefix_without_rewriting_payloads() {
        for event in [
            egui::Event::Paste("alpha".into()),
            egui::Event::Ime(egui::ImeEvent::Commit("alpha".into())),
        ] {
            let ctx = egui::Context::default();
            let mut syntax = PairSyntax::default();
            let mut doc = DocumentSession::new(WindowSessionId::new(1), r"\(\)", DocumentKind::Tex);
            let cursor = frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![egui::Event::Text("\\".into())],
                true,
                CCursorRange::one(CCursor::new(2)),
            );
            frame(&ctx, &mut doc, &mut syntax, vec![event], true, cursor);
            assert_eq!(doc.source(), r"\(\alpha)");
        }
    }

    #[test]
    fn native_tex_completed_closer_processes_chunk_tail_normally() {
        for (source, at, text, expected) in [
            (r"\(\)", 2, ")文", r"\(\)文"),
            (
                r"\begin{文}x\end{文}",
                10,
                "end{文}→",
                r"\begin{文}x\end{文}→",
            ),
        ] {
            let ctx = egui::Context::default();
            let mut syntax = PairSyntax::default();
            let mut doc = DocumentSession::new(WindowSessionId::new(1), source, DocumentKind::Tex);
            let cursor = frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![egui::Event::Text("\\".into())],
                true,
                CCursorRange::one(CCursor::new(at)),
            );
            frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![egui::Event::Text(text.into())],
                true,
                cursor,
            );
            assert_eq!(doc.source(), expected);
        }
    }

    #[test]
    fn native_tex_manual_return_then_backspace_deletes_the_actual_character() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let mut doc = DocumentSession::new(WindowSessionId::new(1), r"\(\)", DocumentKind::Tex);
        frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![
                egui::Event::Text("\\".into()),
                key(egui::Key::ArrowLeft),
                key(egui::Key::ArrowRight),
                key(egui::Key::Backspace),
            ],
            true,
            CCursorRange::one(CCursor::new(2)),
        );
        assert_eq!(doc.source(), r"\()");
    }

    #[test]
    fn native_tex_edit_then_overtype_rekeys_only_its_own_transaction() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let mut doc = DocumentSession::new(WindowSessionId::new(1), r"\(\)", DocumentKind::Tex);
        let cursor = frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![
                egui::Event::Text("x".into()),
                egui::Event::Text("\\".into()),
            ],
            true,
            CCursorRange::one(CCursor::new(2)),
        );
        frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![egui::Event::Text("a".into())],
            true,
            cursor,
        );
        assert_eq!(doc.source(), r"\(x\a\)");
    }

    #[test]
    fn native_tex_enter_completes_a_new_environment_in_one_undo() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let before = "  \\begin{align}";
        let mut doc = DocumentSession::new(WindowSessionId::new(1), before, DocumentKind::Tex);
        let original = CCursorRange::one(CCursor::new(before.chars().count()));
        let cursor = frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![key(egui::Key::Enter)],
            true,
            original,
        );
        assert_eq!(doc.source(), "  \\begin{align}\n    \n  \\end{align}");
        assert_eq!(cursor.primary.index.0, "  \\begin{align}\n    ".len());
        let undo = doc.history_step(false, cursor).unwrap();
        assert_eq!(doc.source(), before);
        assert_eq!(undo.cursor, original);
        doc.history_step(true, undo.cursor).unwrap();
        assert_eq!(doc.source(), "  \\begin{align}\n    \n  \\end{align}");
    }

    #[test]
    fn tex_environment_enter_supports_headers_raw_names_suffixes_and_line_endings() {
        for (before, expected, spaces) in [
            (
                "\\begin{document}|",
                "\\begin{document}\n|\n\\end{document}",
                2,
            ),
            (
                "  \\begin{tabular}{ll}  |",
                "  \\begin{tabular}{ll}  \n    |\n  \\end{tabular}",
                2,
            ),
            (
                "\\begin{minted}[linenos]{tex}|",
                "\\begin{minted}[linenos]{tex}\n  |\n\\end{minted}",
                2,
            ),
            (
                "\\begin{verbatim}|",
                "\\begin{verbatim}\n  |\n\\end{verbatim}",
                2,
            ),
            (
                "\\begin{lstlisting}[language=TeX]|",
                "\\begin{lstlisting}[language=TeX]\n  |\n\\end{lstlisting}",
                2,
            ),
            (
                "\\begin{my:env*}|",
                "\\begin{my:env*}\n  |\n\\end{my:env*}",
                2,
            ),
            ("\\begin{résumé}|", "\\begin{résumé}\n  |\n\\end{résumé}", 2),
            (
                "\\begin{align}|α🙂 suffix\r\nnext",
                "\\begin{align}\r\n  |α🙂 suffix\r\n\\end{align}\r\nnext",
                2,
            ),
            (
                "  text \\begin{align}|x",
                "  text \\begin{align}\n    |x\n  \\end{align}",
                2,
            ),
            (
                " \t\\begin{align}|",
                " \t\\begin{align}\n \t\t|\n \t\\end{align}",
                0,
            ),
            (
                "\\begin{align} % \\begin{fake}|",
                "\\begin{align} % \\begin{fake}\n  |\n\\end{align}",
                2,
            ),
        ] {
            let caret = before.find('|').unwrap();
            let mut source = before.replacen('|', "", 1);
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let mut cursor = CCursor::new(source[..caret].chars().count());
            PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &[key(egui::Key::Enter)],
            )
            .with_document_kind(DocumentKind::Tex)
            .with_indentation(spaces)
            .insert_text_at(&mut cursor, "\n", usize::MAX);
            let byte = source
                .char_indices()
                .nth(cursor.index.0)
                .map_or(source.len(), |(byte, _)| byte);
            source.insert(byte, '|');
            assert_eq!(source, expected, "{before:?}");
        }
    }

    #[test]
    fn tex_environment_enter_deduplicates_real_ends_and_ignores_literal_fake_headers() {
        for (before, expected) in [
            (
                "\\begin{align}|\\end{align}",
                "\\begin{align}\n  |\n\\end{align}",
            ),
            (
                "\\begin{align}|\n  x\n\\end{align}",
                "\\begin{align}\n  |\n  x\n\\end{align}",
            ),
            (
                "\\begin{outer}\n  \\begin{inner}|\n  \\end{inner}",
                "\\begin{outer}\n  \\begin{inner}\n    |\n  \\end{inner}",
            ),
            (
                "\\begin{itemize}\n  \\begin{itemize}|\n\\end{itemize}",
                "\\begin{itemize}\n  \\begin{itemize}\n    |\n  \\end{itemize}\n\\end{itemize}",
            ),
            (
                "\\begin{itemize}\n  \\begin{itemize}|\n  \\end{itemize}\n\\end{itemize}",
                "\\begin{itemize}\n  \\begin{itemize}\n    |\n  \\end{itemize}\n\\end{itemize}",
            ),
            ("% \\begin{align}|", "% \\begin{align}\n|"),
            ("\\url{\\begin{verbatim}|", "\\url{\\begin{verbatim}\n|"),
            (
                "\\begin{verbatim}\n\\begin{align}|",
                "\\begin{verbatim}\n\\begin{align}\n|",
            ),
            ("\\begin{align} x|", "\\begin{align} x\n  |"),
        ] {
            let caret = before.find('|').unwrap();
            let mut source = before.replacen('|', "", 1);
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let mut cursor = CCursor::new(caret);
            PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &[key(egui::Key::Enter)],
            )
            .with_document_kind(DocumentKind::Tex)
            .with_indentation(2)
            .insert_text_at(&mut cursor, "\n", usize::MAX);
            source.insert(cursor.index.0, '|');
            assert_eq!(source, expected, "{before:?}");
        }
    }

    #[test]
    fn tex_environment_completion_preserves_disabled_paste_limit_and_existing_group_behavior() {
        for (events, enabled, limit, expected) in [
            (
                vec![key(egui::Key::Enter)],
                false,
                usize::MAX,
                "\\begin{align}\n  ",
            ),
            (
                vec![egui::Event::Paste("\n".into())],
                true,
                usize::MAX,
                "\\begin{align}\n",
            ),
            (
                vec![key(egui::Key::Enter)],
                true,
                "\\begin{align}".len() + 1,
                "\\begin{align}\n",
            ),
        ] {
            let mut source = "\\begin{align}".to_owned();
            let mut syntax = PairSyntax::default();
            let mut input_state = provisional::State::default();
            let mut cursor = CCursor::new(source.len());
            PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                enabled,
                &events,
            )
            .with_document_kind(DocumentKind::Tex)
            .with_indentation(2)
            .insert_text_at(&mut cursor, "\n", limit);
            assert_eq!(source, expected);
        }
        assert_eq!(
            tex_typed("\\begin{align}", "\\begin{align".len(), "}"),
            ("\\begin{align}".into(), "\\begin{align}".len())
        );
        let mut source = "\\selected".to_owned();
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut buffer = PairingBuffer::new(
            &mut source,
            &mut syntax,
            &mut input_state,
            test_owner(),
            true,
            &[egui::Event::Text("[".into())],
        )
        .with_document_kind(DocumentKind::Tex);
        let mut cursor =
            buffer.delete_selected(&CCursorRange::two(CCursor::new(1), CCursor::new(9)));
        buffer.insert_text_at(&mut cursor, "[", usize::MAX);
        drop(buffer);
        assert_eq!(source, "\\[selected\\]");
    }

    #[test]
    fn native_enter_preserves_existing_indentation_even_without_delimiter_pairing() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            for source in ["  first", "\t  first", "  \t", "\t", "  alpha😀"] {
                let ctx = egui::Context::default();
                let mut syntax = PairSyntax::default();
                let mut doc = DocumentSession::new(WindowSessionId::new(1), source, kind);
                let before_cursor = CCursorRange::one(CCursor::new(source.chars().count()));
                let cursor = frame(
                    &ctx,
                    &mut doc,
                    &mut syntax,
                    vec![key(egui::Key::Enter)],
                    false,
                    before_cursor,
                );
                let indent: String = source
                    .chars()
                    .take_while(|ch| matches!(ch, ' ' | '\t'))
                    .collect();
                let expected = format!("{source}\n{indent}");
                assert_eq!(doc.source(), &expected, "{kind:?}: {source:?}");
                assert_eq!(cursor.primary.index.0, expected.chars().count());
                let undo = doc.history_step(false, cursor).unwrap();
                assert_eq!(doc.source(), source);
                assert_eq!(undo.cursor, before_cursor);
                doc.history_step(true, undo.cursor).unwrap();
                assert_eq!(doc.source(), &expected);
            }
        }
    }

    #[test]
    fn native_enter_handles_structural_closers_and_same_frame_typeahead() {
        for (kind, before, expected) in [
            (DocumentKind::Typst, "#let f() = {}", "#let f() = {\n  x\n}"),
            (
                DocumentKind::Tex,
                "\\newcommand{\\f}{}",
                "\\newcommand{\\f}{\n  x\n}",
            ),
        ] {
            let ctx = egui::Context::default();
            let mut syntax = PairSyntax::default();
            let mut doc = DocumentSession::new(WindowSessionId::new(1), before, kind);
            let before_cursor = CCursorRange::one(CCursor::new(before.chars().count() - 1));
            let cursor = frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![key(egui::Key::Enter), egui::Event::Text("x".into())],
                false,
                before_cursor,
            );
            assert_eq!(doc.source(), expected);
            assert_eq!(cursor.primary.index.0, expected.find("x\n").unwrap() + 1);
            let undo = doc.history_step(false, cursor).unwrap();
            assert_eq!(doc.source(), before);
            doc.history_step(true, undo.cursor).unwrap();
            assert_eq!(doc.source(), expected);
        }
    }

    #[test]
    fn native_enter_between_document_environment_keeps_a_zero_indent_body_line() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let before = "\\begin{document}\\end{document}";
        let mut doc = DocumentSession::new(WindowSessionId::new(1), before, DocumentKind::Tex);
        let cursor = frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![key(egui::Key::Enter)],
            false,
            CCursorRange::one(CCursor::new("\\begin{document}".len())),
        );
        assert_eq!(doc.source(), "\\begin{document}\n\n\\end{document}");
        assert_eq!(cursor.primary.index.0, "\\begin{document}\n".len());
    }

    #[test]
    fn native_enter_replaces_selected_text_with_an_atomic_indented_block() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let before = "#let xs = (old)";
        let mut doc = DocumentSession::new(WindowSessionId::new(1), before, DocumentKind::Typst);
        let start = before.find("old").unwrap();
        let selected = CCursorRange::two(CCursor::new(start), CCursor::new(start + 3));
        let cursor = frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![key(egui::Key::Enter)],
            false,
            selected,
        );
        assert_eq!(doc.source(), "#let xs = (\n  \n)");
        assert_eq!(cursor.primary.index.0, "#let xs = (\n  ".len());
        let undo = doc.history_step(false, cursor).unwrap();
        assert_eq!(doc.source(), before);
        assert_eq!(undo.cursor, selected);
    }

    #[test]
    fn plain_text_enter_and_pasted_or_programmatic_newlines_remain_literal() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let mut input_state = provisional::State::default();
        let mut doc = DocumentSession::new(WindowSessionId::new(1), "  text", DocumentKind::Text);
        frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![key(egui::Key::Enter)],
            false,
            CCursorRange::one(CCursor::new(6)),
        );
        assert_eq!(doc.source(), "  text\n");
        for events in [
            vec![],
            vec![egui::Event::Paste("\n".into())],
            vec![egui::Event::Paste("\n  x\n".into())],
        ] {
            let mut source = "  #let f() = {}".to_owned();
            let mut cursor = CCursor::new(source.chars().count() - 1);
            let text = events
                .first()
                .and_then(|event| {
                    if let egui::Event::Paste(text) = event {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("\n");
            let expected = format!("  #let f() = {{{text}}}");
            PairingBuffer::new(
                &mut source,
                &mut syntax,
                &mut input_state,
                test_owner(),
                true,
                &events,
            )
            .with_indentation(2)
            .insert_text_at(&mut cursor, text, usize::MAX);
            assert_eq!(source, expected);
        }
    }

    #[test]
    fn native_text_edit_preserves_event_order_selection_and_atomic_undo_redo() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let ctx = egui::Context::default();
            let mut syntax = PairSyntax::default();
            let mut doc = DocumentSession::new(WindowSessionId::new(1), "文稿", kind);
            let start = CCursorRange::one(CCursor::new(2));
            let cursor = frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![
                    egui::Event::Text("(".into()),
                    egui::Event::Text("[".into()),
                    key(egui::Key::Backspace),
                ],
                true,
                start,
            );
            assert_eq!(doc.source(), "文稿()");
            assert_eq!(cursor.primary.index.0, 3);
            let undo = doc.history_step(false, cursor).unwrap();
            assert_eq!(doc.source(), "文稿");
            assert_eq!(undo.cursor, start);
            let redo = doc.history_step(true, undo.cursor).unwrap();
            assert_eq!(doc.source(), "文稿()");
            frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![key(egui::Key::Backspace)],
                true,
                redo.cursor,
            );
            assert_eq!(doc.source(), "文稿");
            let selected = CCursorRange::two(CCursor::new(0), CCursor::new(2));
            let cursor = frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![egui::Event::Text("[".into())],
                true,
                selected,
            );
            assert_eq!(doc.source(), "[文稿]");
            assert_eq!(cursor.primary.index.0, 4);
            frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![key(egui::Key::Backspace)],
                false,
                cursor,
            );
            assert_eq!(doc.source(), "[文稿");
        }
    }

    #[test]
    fn selected_text_replacement_stays_verbatim_when_disabled_or_pasted() {
        for (events, enabled, expected) in [
            (vec![egui::Event::Text("(".into())], false, "("),
            (vec![egui::Event::Paste("(".into())], true, "("),
            (vec![egui::Event::Text("word".into())], true, "word"),
            (
                vec![key(egui::Key::Backspace), egui::Event::Text("(".into())],
                true,
                "()",
            ),
        ] {
            let ctx = egui::Context::default();
            let mut syntax = PairSyntax::default();
            let mut doc = DocumentSession::new(WindowSessionId::new(1), "α🦀", DocumentKind::Typst);
            let selected = CCursorRange::two(CCursor::new(2), CCursor::new(0));
            frame(&ctx, &mut doc, &mut syntax, events, enabled, selected);
            assert_eq!(doc.source(), expected);
        }
    }

    #[test]
    fn native_space_after_dollar_keeps_the_caret_between_math_padding() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let mut doc = DocumentSession::new(WindowSessionId::new(1), "", DocumentKind::Typst);
        let mut cursor = frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![egui::Event::Text("$".into())],
            true,
            CCursorRange::one(CCursor::new(0)),
        );
        assert_eq!(doc.source(), "$$");
        assert_eq!(cursor.primary.index.0, 1);
        cursor = frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![egui::Event::Text(" ".into())],
            true,
            cursor,
        );
        assert_eq!(doc.source(), "$  $");
        assert_eq!(cursor.primary.index.0, 2);
    }

    #[test]
    fn native_enter_after_typed_fence_or_dollar_is_one_undoable_edit() {
        for (opening, expected, caret) in [
            ("```tex", "```tex\n\n```", 7),
            ("$", "$\n  \n$", 4),
            ("#mi(```tex", "#mi(```tex\n\n```)", 11),
        ] {
            let ctx = egui::Context::default();
            let mut syntax = PairSyntax::default();
            let mut doc = DocumentSession::new(WindowSessionId::new(1), "", DocumentKind::Typst);
            let mut cursor = CCursorRange::one(CCursor::new(0));
            for ch in opening.chars() {
                cursor = frame(
                    &ctx,
                    &mut doc,
                    &mut syntax,
                    vec![egui::Event::Text(ch.to_string())],
                    true,
                    cursor,
                );
            }
            let before = doc.source().to_owned();
            let previous_cursor = cursor;
            cursor = frame(
                &ctx,
                &mut doc,
                &mut syntax,
                vec![key(egui::Key::Enter)],
                true,
                cursor,
            );
            assert_eq!(doc.source(), expected, "{opening}");
            assert_eq!(cursor.primary.index.0, caret);
            let undo = doc.history_step(false, cursor).unwrap();
            assert_eq!(doc.source(), &before);
            assert_eq!(undo.cursor, previous_cursor);
            doc.history_step(true, undo.cursor).unwrap();
            assert_eq!(doc.source(), expected);
        }
    }

    #[test]
    fn stepping_over_a_closer_is_not_a_document_change() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let mut doc = DocumentSession::new(WindowSessionId::new(1), "()", DocumentKind::Typst);
        let before = doc.key();
        let cursor = frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![egui::Event::Text(")".into())],
            true,
            CCursorRange::one(CCursor::new(1)),
        );
        assert_eq!(cursor.primary.index.0, 2);
        assert_eq!(doc.key(), before);
        assert_eq!(doc.history_availability(), (false, false));
        assert!(doc.take_edit().is_none());
    }

    #[test]
    fn moving_caret_before_typing_and_other_windows_use_current_source() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let mut doc = DocumentSession::new(WindowSessionId::new(1), "中文", DocumentKind::Typst);
        let cursor = frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![key(egui::Key::ArrowLeft), egui::Event::Text("(".into())],
            true,
            CCursorRange::one(CCursor::new(2)),
        );
        assert_eq!(doc.source(), "中(文");
        assert_eq!(cursor.primary.index.0, 2);
        let mut other = DocumentSession::new(WindowSessionId::new(2), "", DocumentKind::Typst);
        frame(
            &ctx,
            &mut other,
            &mut syntax,
            vec![egui::Event::Text("(".into())],
            true,
            CCursorRange::one(CCursor::new(0)),
        );
        assert_eq!(other.source(), "()");
        assert_eq!(doc.source(), "中(文");
    }
}

#[cfg(test)]
mod input_preference_tests {
    use super::*;
    #[test]
    fn punctuation_changes_commits_but_never_preedit_or_paste() {
        let mut events = vec![
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "日本。".into(),
                active_range_chars: None,
            }),
            egui::Event::Ime(egui::ImeEvent::Commit("日本。".into())),
            egui::Event::Paste("。！".into()),
            egui::Event::Text("。！".into()),
        ];
        ascii_punctuation(&mut events);
        assert_eq!(
            events,
            vec![
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "日本。".into(),
                    active_range_chars: None
                }),
                egui::Event::Ime(egui::ImeEvent::Commit("日本.".into())),
                egui::Event::Paste("。！".into()),
                egui::Event::Text(".!".into())
            ]
        );
    }
}
