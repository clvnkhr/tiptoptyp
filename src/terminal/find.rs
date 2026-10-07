//! Find in bounded physical terminal scrollback rows. Queries update locally;
//! only new output requests another worker snapshot, at a bounded rate.

use std::{
    sync::mpsc::{Receiver, TryRecvError, TrySendError},
    time::{Duration, Instant},
};

use super::{
    engine::Cell,
    session::{SearchReply, Session},
};

const DUMP_THROTTLE: Duration = Duration::from_millis(200);
const REPLY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_FIND_MATCHES: usize = 8192;

/// Absolute physical row and original character span, before lowercase can
/// expand a character (for example U+0130 becomes `i` plus a combining dot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FindMatch {
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

/// Case-insensitive substring matches with offsets into the original text.
/// Lowercase byte positions must never be used to slice the original string.
#[cfg(test)]
fn find_matches(dump: &[String], query: &str) -> Vec<FindMatch> {
    find_matches_bounded(dump, query).0
}

/// Final and medial sigma are the same letter for caseless matching. Folding
/// each original character separately still lets us map expansions safely.
fn fold_character(character: char) -> impl Iterator<Item = char> {
    character
        .to_lowercase()
        .map(|lower| if lower == 'ς' { 'σ' } else { lower })
}

fn find_matches_bounded(dump: &[String], query: &str) -> (Vec<FindMatch>, bool) {
    if query.is_empty() {
        return (Vec::new(), false);
    }
    let folded: String = query.chars().flat_map(fold_character).collect();
    let mut matches = Vec::new();
    for (line, text) in dump.iter().enumerate() {
        let mut haystack = String::with_capacity(text.len());
        let mut origins = Vec::with_capacity(text.chars().count());
        for (column, character) in text.chars().enumerate() {
            for lower in fold_character(character) {
                origins.push((haystack.len(), column));
                haystack.push(lower);
            }
        }
        let mut offset = 0;
        while let Some(at) = haystack[offset..].find(&folded) {
            let byte = offset + at;
            let end = byte + folded.len();
            let start_origin = origins.partition_point(|(start, _)| *start < byte);
            let end_origin = origins.partition_point(|(start, _)| *start < end);
            let column = origins[start_origin].1;
            let length = origins[end_origin - 1].1 + 1 - column;
            if matches.len() == MAX_FIND_MATCHES {
                return (matches, true);
            }
            matches.push(FindMatch {
                line,
                column,
                length,
            });
            offset = end;
        }
    }
    (matches, false)
}

struct PendingDump {
    receiver: Receiver<SearchReply>,
    generation: u64,
}

#[derive(Default)]
pub(super) struct TerminalFind {
    open: bool,
    pub(super) query: String,
    matches: Vec<FindMatch>,
    selected: Option<usize>,
    pub(super) dump: Vec<String>,
    first_line: usize,
    truncated: bool,
    match_limit_reached: bool,
    dump_revision: Option<(u64, u64)>,
    pending: Option<PendingDump>,
    last_request: Option<Instant>,
    focus_requested: bool,
    query_dirty: bool,
}

impl TerminalFind {
    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn open(&mut self) {
        self.open = true;
        self.focus_requested = true;
        self.pending = None;
        self.last_request = None;
        self.dump_revision = None;
    }

    pub(super) fn mark_query_dirty(&mut self) {
        self.query_dirty = true;
    }

    pub(super) fn match_count(&self) -> (usize, usize) {
        (self.selected.unwrap_or(0), self.matches.len())
    }

    pub(super) fn matches(&self) -> &[FindMatch] {
        &self.matches
    }
    pub(super) fn selected_index(&self) -> Option<usize> {
        self.selected
    }
    pub(super) fn selected_match(&self) -> Option<FindMatch> {
        self.selected
            .and_then(|index| self.matches.get(index))
            .copied()
    }
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn truncated(&self) -> bool {
        self.truncated || self.match_limit_reached
    }

    pub(super) fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    pub(super) fn consume_focus_request(&mut self) -> bool {
        std::mem::replace(&mut self.focus_requested, false)
    }

    pub(super) fn close(&mut self) {
        self.open = false;
        self.pending = None;
        self.focus_requested = false;
    }

    fn recompute(&mut self) {
        self.recompute_preserving_selection(false);
    }

    fn recompute_preserving_selection(&mut self, preserve: bool) {
        let selected = preserve.then(|| self.selected_match()).flatten();
        (self.matches, self.match_limit_reached) = find_matches_bounded(&self.dump, &self.query);
        for hit in &mut self.matches {
            hit.line += self.first_line;
        }
        self.selected = selected
            .and_then(|selected| self.matches.iter().position(|hit| *hit == selected))
            .or_else(|| (!self.matches.is_empty()).then_some(0));
    }

    fn accept_reply(&mut self, generation: u64, reply: SearchReply) {
        self.dump = reply.dump.lines;
        self.first_line = reply.dump.first_line;
        self.truncated = reply.dump.truncated;
        self.dump_revision = Some((generation, reply.revision));
        self.recompute_preserving_selection(true);
    }

    /// Output refreshes are throttled, while query changes use the retained
    /// dump immediately. Schedule the throttle deadline and wake while a reply
    /// is outstanding; idle results with the same content revision do no work.
    pub(super) fn poll(
        &mut self,
        context: &eframe::egui::Context,
        session: Option<&Session>,
        revision: u64,
    ) {
        self.poll_at(context, session, revision, Instant::now());
    }

    fn poll_at(
        &mut self,
        context: &eframe::egui::Context,
        session: Option<&Session>,
        revision: u64,
        now: Instant,
    ) {
        if !self.open || self.query.is_empty() {
            self.pending = None;
            if self.query.is_empty() {
                self.matches.clear();
                self.selected = None;
                self.query_dirty = false;
            }
            return;
        }
        if std::mem::replace(&mut self.query_dirty, false) {
            self.recompute();
        }
        let Some(session) = session else {
            self.pending = None;
            return;
        };
        let generation = session.generation();
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.generation != generation)
        {
            self.pending = None;
            self.last_request = None;
        }
        if let Some(pending) = &self.pending {
            match pending.receiver.try_recv() {
                Ok(reply) => {
                    self.pending = None;
                    self.accept_reply(generation, reply);
                }
                Err(TryRecvError::Empty) => {
                    context.request_repaint_after(REPLY_POLL_INTERVAL);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                }
            }
        }
        if self.dump_revision == Some((generation, revision)) || !session.can_search() {
            return;
        }
        if let Some(last) = self.last_request {
            let remaining = DUMP_THROTTLE.saturating_sub(now.saturating_duration_since(last));
            if !remaining.is_zero() {
                context.request_repaint_after(remaining);
                return;
            }
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.last_request = Some(now);
        match session.request_search_dump(sender) {
            Ok(()) => {
                self.pending = Some(PendingDump {
                    receiver,
                    generation,
                });
                context.request_repaint_after(REPLY_POLL_INTERVAL);
            }
            Err(TrySendError::Full(_)) => {
                context.request_repaint_after(DUMP_THROTTLE);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Step the selection, wrapping around. Returns the absolute row to reveal.
    pub(super) fn step(&mut self, previous: bool) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let selected = self.selected.unwrap_or(0);
        let next = if previous {
            selected.checked_sub(1).unwrap_or(self.matches.len() - 1)
        } else {
            (selected + 1) % self.matches.len()
        };
        self.selected = Some(next);
        Some(self.matches[next].line)
    }

    pub(super) fn visible_row(
        scroll_offset: usize,
        viewport_rows: usize,
        line: usize,
    ) -> Option<usize> {
        line.checked_sub(scroll_offset)
            .filter(|row| *row < viewport_rows)
    }

    pub(super) fn reveal_delta(
        total_lines: usize,
        viewport_rows: usize,
        scroll_offset: usize,
        line: usize,
    ) -> isize {
        if viewport_rows == 0
            || (scroll_offset..scroll_offset.saturating_add(viewport_rows)).contains(&line)
        {
            return 0;
        }
        let max_offset = total_lines.saturating_sub(viewport_rows);
        line.min(max_offset) as isize - scroll_offset as isize
    }

    /// Original character spans map onto the complete physical cells,
    /// including the spacer tail of a wide glyph and combined graphemes.
    pub(super) fn match_columns(
        cells: &[Cell],
        length: usize,
        column: usize,
    ) -> Option<std::ops::Range<usize>> {
        if length == 0 {
            return None;
        }
        let end_character = column.saturating_add(length);
        let mut chars = 0;
        let mut start_col = None;
        let mut end_col = None;
        for (col, cell) in cells.iter().enumerate() {
            if cell.width == 0 {
                continue;
            }
            let cell_chars = cell.text.chars().count().max(1);
            if chars <= column && column < chars + cell_chars && start_col.is_none() {
                start_col = Some(col);
            }
            if chars < end_character {
                end_col = Some((col + usize::from(cell.width)).min(cells.len()));
            }
            chars += cell_chars;
        }
        match (start_col, end_col) {
            (Some(start), Some(end)) if start < end => Some(start..end),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::engine::SearchDump;
    use super::*;

    #[test]
    fn matches_are_case_insensitive_with_character_columns() {
        let dump = vec![
            "BUILD passed".into(),
            "nothing here".into(),
            "Rebuild done".into(),
        ];
        assert_eq!(
            find_matches(&dump, "build"),
            vec![
                FindMatch {
                    line: 0,
                    column: 0,
                    length: 5
                },
                FindMatch {
                    line: 2,
                    column: 2,
                    length: 5
                },
            ]
        );
        assert!(find_matches(&dump, "").is_empty());
        assert!(find_matches(&dump, "missing").is_empty());
    }

    #[test]
    fn lowercase_expansion_maps_back_to_original_grapheme_positions() {
        let dump = vec!["İXİ".into(), "aȺX".into(), "界Éx".into()];
        assert_eq!(
            find_matches(&dump, "x"),
            vec![
                FindMatch {
                    line: 0,
                    column: 1,
                    length: 1
                },
                FindMatch {
                    line: 1,
                    column: 2,
                    length: 1
                },
                FindMatch {
                    line: 2,
                    column: 2,
                    length: 1
                },
            ]
        );
        assert_eq!(
            find_matches(&dump, "İx"),
            vec![FindMatch {
                line: 0,
                column: 0,
                length: 2
            }]
        );
        assert_eq!(
            find_matches(&dump, "i"),
            vec![
                FindMatch {
                    line: 0,
                    column: 0,
                    length: 1
                },
                FindMatch {
                    line: 0,
                    column: 2,
                    length: 1
                },
            ]
        );
    }

    #[test]
    fn greek_final_sigma_is_caseless_with_original_offsets_after_expansion() {
        let dump = vec!["ΟΣ Σ σ ς".into(), "İςX".into()];
        assert_eq!(
            find_matches(&dump, "ος"),
            vec![FindMatch {
                line: 0,
                column: 0,
                length: 2
            }]
        );
        assert_eq!(
            find_matches(&dump, "σ"),
            vec![
                FindMatch {
                    line: 0,
                    column: 1,
                    length: 1
                },
                FindMatch {
                    line: 0,
                    column: 3,
                    length: 1
                },
                FindMatch {
                    line: 0,
                    column: 5,
                    length: 1
                },
                FindMatch {
                    line: 0,
                    column: 7,
                    length: 1
                },
                FindMatch {
                    line: 1,
                    column: 1,
                    length: 1
                },
            ]
        );
        assert_eq!(
            find_matches(&dump, "ςx"),
            vec![FindMatch {
                line: 1,
                column: 1,
                length: 2
            }]
        );
    }

    #[test]
    fn matching_work_and_retained_hits_are_bounded() {
        let dump = vec!["a".repeat(MAX_FIND_MATCHES + 50)];
        let (hits, truncated) = find_matches_bounded(&dump, "a");
        assert_eq!(hits.len(), MAX_FIND_MATCHES);
        assert!(truncated);
    }

    #[test]
    fn capped_dump_keeps_absolute_coordinates_and_selected_hit_on_refresh() {
        let mut find = TerminalFind::default();
        find.open();
        find.query = "match".into();
        find.accept_reply(
            7,
            SearchReply {
                revision: 4,
                dump: SearchDump {
                    lines: vec!["match".into(), "match".into()],
                    first_line: 90,
                    truncated: true,
                },
            },
        );
        assert_eq!(find.step(false), Some(91));
        find.accept_reply(
            7,
            SearchReply {
                revision: 5,
                dump: SearchDump {
                    lines: vec!["match".into(), "match".into(), "match".into()],
                    first_line: 90,
                    truncated: true,
                },
            },
        );
        assert_eq!(find.selected_match().unwrap().line, 91);
        assert_eq!(find.dump_revision, Some((7, 5)));
        assert!(find.truncated());
        find.query = "missing".into();
        find.recompute();
        assert_eq!(find.selected_match(), None);
    }

    #[test]
    fn reply_revision_is_the_snapshot_revision_even_when_new_output_arrives() {
        let mut find = TerminalFind {
            query: "x".into(),
            ..Default::default()
        };
        find.accept_reply(
            8,
            SearchReply {
                revision: 2,
                dump: SearchDump {
                    lines: vec!["x".into()],
                    ..Default::default()
                },
            },
        );
        assert_ne!(find.dump_revision, Some((8, 3)));
    }

    #[test]
    fn wide_and_combining_matches_cover_complete_cells() {
        let cell = |text: &str, width| Cell {
            text: text.into(),
            width,
            foreground: eframe::egui::Color32::WHITE,
            background: eframe::egui::Color32::BLACK,
            style: Default::default(),
        };
        let cells = vec![
            cell("界", 2),
            cell("", 0),
            cell("e\u{301}", 1),
            cell("x", 1),
        ];
        assert_eq!(TerminalFind::match_columns(&cells, 1, 0), Some(0..2));
        assert_eq!(TerminalFind::match_columns(&cells, 1, 2), Some(2..3));
        assert_eq!(TerminalFind::match_columns(&cells, 1, 3), Some(3..4));
    }

    #[test]
    fn old_reply_keeps_newer_output_dirty_and_idle_results_never_request_again() {
        let context = eframe::egui::Context::default();
        let (session, commands) = Session::search_test_session();
        let now = Instant::now();
        let mut find = TerminalFind::default();
        find.open();
        find.query = "x".into();
        find.poll_at(&context, Some(&session), 1, now);
        let super::super::session::Command::SearchDump { reply } = commands.try_recv().unwrap()
        else {
            panic!("expected dump")
        };
        reply
            .send(SearchReply {
                revision: 1,
                dump: SearchDump {
                    lines: vec!["x".into()],
                    ..Default::default()
                },
            })
            .unwrap();
        find.poll_at(&context, Some(&session), 2, now + Duration::from_millis(50));
        assert_eq!(find.dump_revision, Some((session.generation(), 1)));
        assert!(commands.try_recv().is_err());
        find.poll_at(&context, Some(&session), 2, now + DUMP_THROTTLE);
        let super::super::session::Command::SearchDump { reply } = commands.try_recv().unwrap()
        else {
            panic!("expected updated dump")
        };
        reply
            .send(SearchReply {
                revision: 2,
                dump: SearchDump {
                    lines: vec!["x x".into()],
                    ..Default::default()
                },
            })
            .unwrap();
        for _ in 0..10 {
            find.poll_at(&context, Some(&session), 2, now + DUMP_THROTTLE);
        }
        assert_eq!(find.match_count().1, 2);
        assert!(!find.pending());
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn disconnected_reply_clears_pending_and_restart_rejects_old_generation() {
        let context = eframe::egui::Context::default();
        let (old_session, old_commands) = Session::search_test_session();
        let (new_session, new_commands) = Session::search_test_session();
        let now = Instant::now();
        let mut find = TerminalFind::default();
        find.open();
        find.query = "x".into();
        find.poll_at(&context, Some(&old_session), 0, now);
        let super::super::session::Command::SearchDump { reply } = old_commands.try_recv().unwrap()
        else {
            panic!("expected dump")
        };
        reply
            .send(SearchReply {
                revision: 0,
                dump: SearchDump {
                    lines: vec!["x old".into()],
                    ..Default::default()
                },
            })
            .unwrap();
        find.poll_at(&context, Some(&new_session), 0, now);
        assert!(
            find.dump.is_empty(),
            "an old session must not populate the new pane"
        );
        assert!(find.pending());
        let command = new_commands.try_recv().unwrap();
        drop(command); // worker disappeared before its reply
        find.poll_at(
            &context,
            Some(&new_session),
            0,
            now + Duration::from_millis(20),
        );
        assert!(
            !find.pending(),
            "a disconnected receiver must not remain pending forever"
        );
        find.poll_at(&context, Some(&new_session), 0, now + DUMP_THROTTLE);
        assert!(matches!(
            new_commands.try_recv(),
            Ok(super::super::session::Command::SearchDump { .. })
        ));
        find.close();
        assert!(!find.pending());
    }

    #[test]
    fn viewport_mapping_and_reveal_math_hold() {
        assert_eq!(TerminalFind::visible_row(2, 4, 0), None);
        assert_eq!(TerminalFind::visible_row(2, 4, 2), Some(0));
        assert_eq!(TerminalFind::visible_row(2, 4, 5), Some(3));
        assert_eq!(TerminalFind::visible_row(2, 4, 6), None);
        assert_eq!(TerminalFind::reveal_delta(10, 4, 0, 9), 6);
        assert_eq!(TerminalFind::reveal_delta(10, 4, 0, 0), 0);
        assert_eq!(TerminalFind::reveal_delta(10, 4, 6, 9), 0);
        assert_eq!(TerminalFind::reveal_delta(10, 4, 6, 2), -4);
        assert_eq!(TerminalFind::reveal_delta(3, 4, 0, 2), 0);
    }

    #[test]
    fn stepping_wraps_around_the_match_list() {
        let mut find = TerminalFind::default();
        find.open();
        find.dump = vec!["a".into(), "b".into(), "a".into()];
        find.query = "a".into();
        find.recompute();
        assert_eq!(find.step(false), Some(2));
        assert_eq!(find.step(false), Some(0));
        assert_eq!(find.step(true), Some(2));
    }
}
