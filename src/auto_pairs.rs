//! Adapt native TextEdit operations, keeping event ordering, selections, IME,
//! and undo in their existing owners. Only explicit typing enables pairing.
use std::ops::Range;

use eframe::egui::{
    self, TextBuffer,
    text::{CCursor, CharIndex},
};
use tiptoptyp_core::pairing::{self, TypedAction};
use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

pub(crate) struct PairSyntax(Source);

impl Default for PairSyntax {
    fn default() -> Self {
        Self(Source::detached(String::new()))
    }
}

impl PairSyntax {
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

    pub(crate) fn newline(&mut self, source: &str, byte: usize) -> Option<(String, usize)> {
        let line_ending = if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let line_start = source[..byte].rfind('\n').map_or(0, |at| at + 1);
        let prefix = &source[line_start..byte];
        let indent_len = prefix
            .bytes()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        let indent = &prefix[..indent_len];
        if source[..byte].ends_with('$') && source[byte..].starts_with('$') {
            self.prepare(source);
            if self.empty_at(source, byte) {
                let body = format!("{line_ending}{indent}  ");
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

pub(crate) struct PairingBuffer<'a> {
    source: &'a mut String,
    syntax: &'a mut PairSyntax,
    enabled: bool,
}

impl<'a> PairingBuffer<'a> {
    pub(crate) fn new(
        source: &'a mut String,
        syntax: &'a mut PairSyntax,
        enabled: bool,
        events: &[egui::Event],
    ) -> Self {
        // Composition and pasted batches are verbatim, even if a platform
        // also emits Text alongside them. Never pair transient IME preedit.
        let verbatim = events
            .iter()
            .any(|event| matches!(event, egui::Event::Paste(_) | egui::Event::Ime(_)));
        Self {
            source,
            syntax,
            enabled: enabled && !verbatim,
        }
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
        self.source.insert_text(text, index)
    }
    fn delete_char_range(&mut self, range: Range<CharIndex>) {
        self.source.delete_char_range(range);
    }
    fn insert_text_at(&mut self, cursor: &mut CCursor, text: &str, limit: usize) {
        if self.enabled && text == "\n" {
            let byte = self.source.byte_index_from_char_index(cursor.index).0;
            if let Some((inserted, advance)) = self.syntax.newline(self.source, byte)
                && limit.saturating_sub(self.source.chars().count()) >= inserted.chars().count()
            {
                self.source.insert_text(&inserted, cursor.index);
                cursor.index += advance;
                return;
            }
        }
        if self.enabled && text == " " {
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
        if self.enabled && text == "`" {
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
            self.syntax.prepare(self.source);
            let step = next == Some(typed)
                && (self.syntax.may_step_over(byte, typed)
                    || self.syntax.empty_at(self.source, byte));
            self.syntax.prepare(self.source);
            let open = !step && self.syntax.may_open(self.source, byte, typed);
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
    fn delete_previous_char(&mut self, cursor: CCursor) -> CCursor {
        if self.enabled && cursor.index.0 > 0 {
            let byte = self.source.byte_index_from_char_index(cursor.index).0;
            let before = self.source[..byte].chars().next_back();
            let after = self.source[byte..].chars().next();
            if pairing::empty_pair(before, after) {
                self.syntax.prepare(self.source);
                if self.syntax.empty_at(self.source, byte) {
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

    fn typed(source: &str, caret: usize, text: &str) -> (String, usize) {
        let mut source = source.to_owned();
        let mut syntax = PairSyntax::default();
        let mut cursor = CCursor::new(caret);
        PairingBuffer::new(&mut source, &mut syntax, true, &[]).insert_text_at(
            &mut cursor,
            text,
            usize::MAX,
        );
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
            let cursor = PairingBuffer::new(&mut expected, &mut syntax, true, &[])
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
        let mut cursor = CCursor::new(caret);
        for typed in text.chars() {
            PairingBuffer::new(&mut source, &mut syntax, true, &[]).insert_text_at(
                &mut cursor,
                &typed.to_string(),
                usize::MAX,
            );
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
            let mut cursor = CCursor::new(1);
            PairingBuffer::new(&mut source, &mut syntax, enabled, &events).insert_text_at(
                &mut cursor,
                "\n",
                limit,
            );
            assert_eq!(source, "$\n$");
            assert_eq!(cursor.index.0, 2);
        }
    }

    #[test]
    fn ordinary_enter_does_not_parse() {
        let mut source = "ordinary text".to_owned();
        let mut syntax = PairSyntax::default();
        let mut cursor = CCursor::new(source.chars().count());
        PairingBuffer::new(&mut source, &mut syntax, true, &[]).insert_text_at(
            &mut cursor,
            "\n",
            usize::MAX,
        );
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
            PairingBuffer::new(&mut source, &mut syntax, true, &[event]).insert_text_at(
                &mut CCursor::new(0),
                "(",
                usize::MAX,
            );
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
        let mut source = String::new();
        let mut cursor = CCursor::new(0);
        PairingBuffer::new(&mut source, &mut syntax, true, &[]).insert_text_at(
            &mut cursor,
            "文",
            usize::MAX,
        );
        assert!(syntax.0.text().is_empty());
        PairingBuffer::new(&mut source, &mut syntax, true, &[]).insert_text_at(&mut cursor, "(", 2);
        assert_eq!(source, "文(");
        assert_eq!(cursor.index.0, 2);
        PairingBuffer::new(&mut source, &mut syntax, true, &[]).insert_text_at(&mut cursor, "[", 2);
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
        ctx.run_ui(input, |ui| {
            document.edit(selection, |source| {
                let mut buffer = PairingBuffer::new(source, syntax, enabled, &events);
                let output = egui::TextEdit::multiline(&mut buffer)
                    .code_editor()
                    .id(id)
                    .show(ui);
                result = output.state.cursor.char_range().unwrap();
            });
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
    fn native_text_edit_preserves_event_order_selection_and_atomic_undo_redo() {
        let ctx = egui::Context::default();
        let mut syntax = PairSyntax::default();
        let mut doc = DocumentSession::new(WindowSessionId::new(1), "文稿", DocumentKind::Typst);
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
        assert_eq!(doc.source(), "[]");
        assert_eq!(cursor.primary.index.0, 1);
        frame(
            &ctx,
            &mut doc,
            &mut syntax,
            vec![key(egui::Key::Backspace)],
            false,
            cursor,
        );
        assert_eq!(doc.source(), "]");
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
