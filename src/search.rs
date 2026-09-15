use std::ops::{Deref, Range};

/// A match expressed in both of the coordinate systems used by the
/// editor: UTF-8 byte offsets for editing `String`s and Unicode scalar offsets
/// for positioning an egui text cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub byte_range: Range<usize>,
    pub char_range: Range<usize>,
}

impl SearchMatch {
    /// Build a match from a byte range, rejecting ranges which are out of
    /// bounds, reversed, or not on UTF-8 character boundaries.
    #[cfg(test)]
    fn from_byte_range(text: &str, byte_range: Range<usize>) -> Option<Self> {
        if byte_range.start > byte_range.end
            || byte_range.end > text.len()
            || !text.is_char_boundary(byte_range.start)
            || !text.is_char_boundary(byte_range.end)
        {
            return None;
        }

        let char_start = text[..byte_range.start].chars().count();
        let char_len = text[byte_range.clone()].chars().count();
        Some(Self {
            byte_range,
            char_range: char_start..char_start + char_len,
        })
    }
}

use crate::document::DocumentKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    InvalidRegex(String),
}

/// A completed search. Invalid patterns are deliberately distinguishable from
/// valid searches with no matches.
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    matches: Vec<SearchMatch>,
    error: Option<SearchError>,
}

impl SearchResults {
    pub fn error(&self) -> Option<&SearchError> {
        self.error.as_ref()
    }
}

impl Deref for SearchResults {
    type Target = [SearchMatch];

    fn deref(&self) -> &Self::Target {
        &self.matches
    }
}

/// Find all non-overlapping, case-sensitive literal matches.
///
/// Empty queries deliberately have no matches. This matches editor find
/// behavior and avoids treating every character boundary as a result.
fn find_all(text: &str, query: &str) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let query_chars = query.chars().count();
    let mut previous_byte_end = 0;
    let mut previous_char_end = 0;
    text.match_indices(query)
        .map(|(start, matched)| {
            let char_start = previous_char_end + text[previous_byte_end..start].chars().count();
            let byte_end = start + matched.len();
            let search_match = SearchMatch {
                byte_range: start..byte_end,
                char_range: char_start..char_start + query_chars,
            };
            previous_byte_end = byte_end;
            previous_char_end = search_match.char_range.end;
            search_match
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryKey {
    query: String,
    case_sensitive: bool,
    regex: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchKey {
    document: DocumentKey,
    query: QueryKey,
}

enum CompiledQuery {
    Empty,
    Literal(String),
    FoldedLiteral(String),
    Regex(regex_automata::meta::Regex),
    InvalidRegex(String),
}

impl CompiledQuery {
    fn compile(key: &QueryKey) -> Self {
        if key.query.is_empty() {
            return Self::Empty;
        }
        if !key.regex {
            return if key.case_sensitive {
                Self::Literal(key.query.clone())
            } else {
                Self::FoldedLiteral(fold(&key.query))
            };
        }
        match regex_automata::meta::Regex::builder()
            .syntax(
                regex_automata::util::syntax::Config::new().case_insensitive(!key.case_sensitive),
            )
            .build(&key.query)
        {
            Ok(pattern) => Self::Regex(pattern),
            Err(error) => Self::InvalidRegex(error.to_string()),
        }
    }

    fn find(&self, text: &str) -> SearchResults {
        let matches = match self {
            Self::Empty | Self::InvalidRegex(_) => Vec::new(),
            Self::Literal(query) => find_all(text, query),
            Self::FoldedLiteral(query) => find_all_folded(text, query),
            Self::Regex(pattern) => find_all_regex(text, pattern),
        };
        let error = match self {
            Self::InvalidRegex(error) => Some(SearchError::InvalidRegex(error.clone())),
            _ => None,
        };
        SearchResults { matches, error }
    }
}

/// Stateful, revision-keyed search data shared by the count, navigation, and
/// replacement controls.
pub struct SearchSession {
    key: Option<SearchKey>,
    compiled_key: Option<QueryKey>,
    query: CompiledQuery,
    results: SearchResults,
    selected: Option<usize>,
    anchored_selection: Option<SearchMatch>,
    #[cfg(test)]
    compile_count: usize,
    #[cfg(test)]
    scan_count: usize,
}

impl Default for SearchSession {
    fn default() -> Self {
        Self {
            key: None,
            compiled_key: None,
            query: CompiledQuery::Empty,
            results: SearchResults::default(),
            selected: None,
            anchored_selection: None,
            #[cfg(test)]
            compile_count: 0,
            #[cfg(test)]
            scan_count: 0,
        }
    }
}

impl SearchSession {
    pub fn results(
        &mut self,
        text: &str,
        document: DocumentKey,
        query: &str,
        case_sensitive: bool,
        regex: bool,
    ) -> &SearchResults {
        self.prepare(text, document, query, case_sensitive, regex);
        &self.results
    }

    pub fn selected(&self) -> Option<&SearchMatch> {
        self.anchored_selection.as_ref().or_else(|| {
            self.selected
                .and_then(|selected| self.results.get(selected))
        })
    }

    /// Clear navigation without discarding a still-valid compiled query or
    /// match result.
    pub fn clear(&mut self) {
        self.selected = None;
        self.anchored_selection = None;
    }

    pub fn next(
        &mut self,
        text: &str,
        document: DocumentKey,
        query: &str,
        case_sensitive: bool,
        regex: bool,
    ) -> Option<&SearchMatch> {
        self.prepare(text, document, query, case_sensitive, regex);
        self.anchored_selection = None;
        self.selected = (!self.results.is_empty()).then(|| {
            self.selected
                .map_or(0, |selected| (selected + 1) % self.results.len())
        });
        self.selected()
    }

    pub fn previous(
        &mut self,
        text: &str,
        document: DocumentKey,
        query: &str,
        case_sensitive: bool,
        regex: bool,
    ) -> Option<&SearchMatch> {
        self.prepare(text, document, query, case_sensitive, regex);
        self.anchored_selection = None;
        self.selected = (!self.results.is_empty()).then(|| {
            self.selected.map_or(self.results.len() - 1, |selected| {
                selected.checked_sub(1).unwrap_or(self.results.len() - 1)
            })
        });
        self.selected()
    }

    /// Replace the selected match, or the first match when none is selected.
    /// The revised text is scanned exactly once and the next result is selected.
    pub fn replace_one(
        &mut self,
        text: &mut String,
        document: DocumentKey,
        query: &str,
        replacement: &str,
        case_sensitive: bool,
        regex: bool,
    ) -> bool {
        self.prepare(text, document, query, case_sensitive, regex);
        let Some(matched) = self
            .selected()
            .cloned()
            .or_else(|| self.results.first().cloned())
        else {
            self.selected = None;
            return false;
        };
        let empty_noop_match = matched.byte_range.is_empty() && replacement.is_empty();
        let changed = &text[matched.byte_range.clone()] != replacement;
        let anchor = matched.byte_range.start + replacement.len();
        text.replace_range(matched.byte_range, replacement);
        let next_document = if changed {
            document.after_edit()
        } else {
            document
        };
        self.prepare(text, next_document, query, case_sensitive, regex);
        if case_sensitive && !regex {
            // Preserve the editor's anchor semantics: a match may begin at the
            // end of the replacement even when it overlaps the normal
            // non-overlapping result sequence. Wrapping only examines the
            // prefix, so a match straddling the anchor is not selected.
            let anchored = find_at_or_after(text, query, anchor).or_else(|| {
                text.get(..anchor)
                    .and_then(|prefix| find_at_or_after(prefix, query, 0))
            });
            self.selected = anchored.as_ref().and_then(|anchored| {
                self.results
                    .iter()
                    .position(|matched| matched.byte_range == anchored.byte_range)
            });
            self.anchored_selection = if self.selected.is_none() {
                anchored
            } else {
                None
            };
        } else {
            self.selected = self
                .results
                .iter()
                .position(|matched| {
                    if empty_noop_match {
                        matched.byte_range.start > anchor
                    } else {
                        matched.byte_range.start >= anchor
                    }
                })
                .or((!self.results.is_empty()).then_some(0));
        }
        true
    }

    /// Replace the cached, original non-overlapping result set. Replacement
    /// text is literal even in regex mode.
    pub fn replace_all(
        &mut self,
        text: &mut String,
        document: DocumentKey,
        query: &str,
        replacement: &str,
        case_sensitive: bool,
        regex: bool,
    ) -> usize {
        self.prepare(text, document, query, case_sensitive, regex);
        self.selected = None;
        self.anchored_selection = None;
        if self.results.is_empty() {
            return 0;
        }
        let matches = self.results.matches.clone();
        let mut result = String::with_capacity(text.len());
        let mut copied_until = 0;
        for matched in &matches {
            result.push_str(&text[copied_until..matched.byte_range.start]);
            result.push_str(replacement);
            copied_until = matched.byte_range.end;
        }
        result.push_str(&text[copied_until..]);
        let changed = result != *text;
        *text = result;
        let next_document = if changed {
            document.after_edit()
        } else {
            document
        };
        self.prepare(text, next_document, query, case_sensitive, regex);
        self.selected = None;
        self.anchored_selection = None;
        matches.len()
    }

    fn prepare(
        &mut self,
        text: &str,
        document: DocumentKey,
        query: &str,
        case_sensitive: bool,
        regex: bool,
    ) {
        let _span = crate::performance::span("search.total");
        let same_key = self.key.as_ref().is_some_and(|key| {
            key.document == document
                && key.query.query == query
                && key.query.case_sensitive == case_sensitive
                && key.query.regex == regex
        });
        if same_key {
            return;
        }
        let _rebuild = crate::performance::span("search.rebuild");
        let same_query = self.compiled_key.as_ref().is_some_and(|key| {
            key.query == query && key.case_sensitive == case_sensitive && key.regex == regex
        });
        if !same_query {
            let query_key = QueryKey {
                query: query.to_owned(),
                case_sensitive,
                regex,
            };
            self.query = CompiledQuery::compile(&query_key);
            self.compiled_key = Some(query_key);
            #[cfg(test)]
            {
                self.compile_count += 1;
            }
        }
        self.results = self.query.find(text);
        self.key = Some(SearchKey {
            document,
            query: self.compiled_key.clone().unwrap_or(QueryKey {
                query: query.to_owned(),
                case_sensitive,
                regex,
            }),
        });
        self.selected = None;
        self.anchored_selection = None;
        #[cfg(test)]
        {
            self.scan_count += 1;
        }
    }

    #[cfg(test)]
    fn work_counts(&self) -> (usize, usize) {
        (self.compile_count, self.scan_count)
    }
}

#[cfg(test)]
fn find_all_with_options(
    text: &str,
    query: &str,
    case_sensitive: bool,
    regex: bool,
) -> SearchResults {
    CompiledQuery::compile(&QueryKey {
        query: query.to_owned(),
        case_sensitive,
        regex,
    })
    .find(text)
}

fn find_all_folded(text: &str, query: &str) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let folded = FoldedText::new(text);
    folded
        .text
        .match_indices(query)
        .filter_map(|(start, matched)| folded.source_range(start..start + matched.len()))
        .collect()
}

fn find_all_regex(text: &str, pattern: &regex_automata::meta::Regex) -> Vec<SearchMatch> {
    let mut byte_cursor = 0;
    let mut char_cursor = 0;
    pattern
        .find_iter(text)
        .map(|matched| {
            char_cursor += text[byte_cursor..matched.start()].chars().count();
            let char_start = char_cursor;
            char_cursor += text[matched.range()].chars().count();
            byte_cursor = matched.end();
            SearchMatch {
                byte_range: matched.range(),
                char_range: char_start..char_cursor,
            }
        })
        .collect()
}

fn fold(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

fn find_at_or_after(text: &str, query: &str, start: usize) -> Option<SearchMatch> {
    if query.is_empty() {
        return None;
    }
    let suffix = text.get(start..)?;
    let offset = suffix.find(query)?;
    let byte_start = start + offset;
    let byte_range = byte_start..byte_start + query.len();
    let char_start = text[..byte_start].chars().count();
    let char_len = text[byte_range.clone()].chars().count();
    Some(SearchMatch {
        byte_range,
        char_range: char_start..char_start + char_len,
    })
}

struct FoldedText {
    text: String,
    /// Boundaries after each original scalar: folded byte, source byte, and
    /// source scalar offsets. Matches which split a scalar's folded expansion
    /// are intentionally rejected.
    boundaries: Vec<(usize, usize, usize)>,
}

impl FoldedText {
    fn new(source: &str) -> Self {
        let mut text = String::with_capacity(source.len());
        let mut boundaries = Vec::with_capacity(source.chars().count() + 1);
        boundaries.push((0, 0, 0));
        for (character_index, (byte, character)) in source.char_indices().enumerate() {
            text.extend(character.to_lowercase());
            boundaries.push((text.len(), byte + character.len_utf8(), character_index + 1));
        }
        Self { text, boundaries }
    }

    fn source_range(&self, folded: Range<usize>) -> Option<SearchMatch> {
        let start = self
            .boundaries
            .binary_search_by_key(&folded.start, |boundary| boundary.0)
            .ok()?;
        let end = self
            .boundaries
            .binary_search_by_key(&folded.end, |boundary| boundary.0)
            .ok()?;
        Some(SearchMatch {
            byte_range: self.boundaries[start].1..self.boundaries[end].1,
            char_range: self.boundaries[start].2..self.boundaries[end].2,
        })
    }
}

/// Find the match after `selected`, wrapping to the first match at the end.
/// A stale or unrelated selection is ignored.
#[cfg(test)]
fn find_next_with_options(
    text: &str,
    query: &str,
    selected: Option<&SearchMatch>,
    case_sensitive: bool,
    regex: bool,
) -> Option<SearchMatch> {
    let matches = find_all_with_options(text, query, case_sensitive, regex);
    let selected = selected.filter(|selected| {
        matches
            .iter()
            .any(|matched| matched.byte_range == selected.byte_range)
    });
    selected
        .and_then(|selected| {
            matches
                .iter()
                .find(|matched| {
                    matched.byte_range != selected.byte_range
                        && matched.byte_range.start >= selected.byte_range.end
                })
                .cloned()
        })
        .or_else(|| matches.first().cloned())
}

/// Find the match before `selected`, wrapping to the final match at the start.
/// A stale or unrelated selection starts at the final match.
#[cfg(test)]
fn find_previous_with_options(
    text: &str,
    query: &str,
    selected: Option<&SearchMatch>,
    case_sensitive: bool,
    regex: bool,
) -> Option<SearchMatch> {
    let matches = find_all_with_options(text, query, case_sensitive, regex);
    let selected = selected.filter(|selected| {
        matches
            .iter()
            .any(|matched| matched.byte_range == selected.byte_range)
    });
    selected
        .and_then(|selected| {
            matches
                .iter()
                .rfind(|matched| {
                    matched.byte_range != selected.byte_range
                        && matched.byte_range.end <= selected.byte_range.start
                })
                .cloned()
        })
        .or_else(|| matches.last().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision(revision: u64) -> DocumentKey {
        DocumentKey::new(
            tiptoptyp_core::document::WindowSessionId::new(1),
            7,
            revision,
        )
    }

    fn byte_ranges(matches: &[SearchMatch]) -> Vec<Range<usize>> {
        matches
            .iter()
            .map(|matched| matched.byte_range.clone())
            .collect()
    }

    fn char_ranges(matches: &[SearchMatch]) -> Vec<Range<usize>> {
        matches
            .iter()
            .map(|matched| matched.char_range.clone())
            .collect()
    }

    #[test]
    fn literal_find_is_case_sensitive_and_non_overlapping() {
        let matches = find_all_with_options("Aa.a aa", "a", true, false);
        assert_eq!(byte_ranges(&matches), vec![1..2, 3..4, 5..6, 6..7]);

        let overlapping = find_all_with_options("aaaa", "aa", true, false);
        assert_eq!(byte_ranges(&overlapping), vec![0..2, 2..4]);
    }

    #[test]
    fn case_insensitive_find_preserves_unicode_ranges() {
        let matches = find_all_with_options("Alpha ALPHA 🦀", "alpha", false, false);
        assert_eq!(byte_ranges(&matches), vec![0..5, 6..11]);
        assert_eq!(char_ranges(&matches), vec![0..5, 6..11]);
    }

    #[test]
    fn editor_regex_supports_classes_quantifiers_and_anchors() {
        let matches = find_all_with_options("item-12", r"^item-\d+$", true, true);
        assert_eq!(byte_ranges(&matches), vec![0..7]);
        let matches = find_all_with_options("cat cot cut", r"c.t", true, true);
        assert_eq!(matches.len(), 3);
        let matches = find_all_with_options("a12 b7", r"\w+\d+", true, true);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn invalid_editor_regex_fails_closed() {
        let invalid = find_all_with_options("abc", "[", true, true);
        assert!(invalid.is_empty());
        assert!(matches!(
            invalid.error(),
            Some(SearchError::InvalidRegex(_))
        ));
        let empty = find_all_with_options("abc", "", true, true);
        assert!(empty.is_empty());
        assert_eq!(empty.error(), None);
    }

    #[test]
    fn unchanged_revision_reuses_compilation_and_scan() {
        let mut session = SearchSession::default();
        let text = "one two one";

        assert_eq!(
            session.results(text, revision(3), "one", true, false).len(),
            2
        );
        assert_eq!(
            session.results(text, revision(3), "one", true, false).len(),
            2
        );
        session.next(text, revision(3), "one", true, false);
        session.previous(text, revision(3), "one", true, false);
        assert_eq!(session.work_counts(), (1, 1));

        assert_eq!(
            session
                .results("one", revision(4), "one", true, false)
                .len(),
            1
        );
        assert_eq!(session.work_counts(), (1, 2));
        assert_eq!(
            session
                .results("two", revision(4), "two", true, false)
                .len(),
            1
        );
        assert_eq!(session.work_counts(), (2, 3));
    }

    #[test]
    fn replacement_rescans_new_revision_once_and_reuses_compiled_query() {
        let mut session = SearchSession::default();
        let mut text = "one two one".to_owned();
        session.next(&text, revision(0), "one", true, false);
        assert!(session.replace_one(&mut text, revision(0), "one", "1", true, false,));
        assert_eq!(text, "1 two one");
        assert_eq!(session.work_counts(), (1, 2));
        assert_eq!(
            session
                .results(&text, revision(1), "one", true, false)
                .len(),
            1
        );
        assert_eq!(session.work_counts(), (1, 2));
    }

    #[test]
    fn folded_matching_maps_only_complete_source_scalars() {
        let source = "İ i\u{307}";
        let lowercase_i = find_all_with_options(source, "i", false, false);
        assert_eq!(byte_ranges(&lowercase_i), vec![3..4]);
        assert_eq!(char_ranges(&lowercase_i), vec![2..3]);

        let expanded = find_all_with_options(source, "i\u{307}", false, false);
        assert_eq!(byte_ranges(&expanded), vec![0..2, 3..6]);
        assert_eq!(char_ranges(&expanded), vec![0..1, 2..4]);
    }

    #[test]
    fn regex_handles_unicode_case_ranges_and_standard_escapes() {
        assert_eq!(
            byte_ranges(&find_all_with_options("M", "[a-z]", false, true)),
            vec![0..1]
        );
        assert_eq!(
            byte_ranges(&find_all_with_options("É", "é", false, true)),
            vec![0..2]
        );
        assert_eq!(
            byte_ranges(&find_all_with_options("a\nb", r"\n", true, true)),
            vec![1..2]
        );
        assert!(find_all_with_options("*", "*", true, true).is_empty());
        assert!(find_all_with_options("b", "[z-a]", true, true).is_empty());
    }

    #[test]
    fn zero_width_regex_navigation_advances_and_wraps() {
        let mut state = SearchSession::default();
        let text = "é x";
        assert_eq!(
            state
                .next(text, revision(0), r"\b", true, true)
                .unwrap()
                .byte_range,
            0..0
        );
        assert_eq!(
            state
                .next(text, revision(0), r"\b", true, true)
                .unwrap()
                .byte_range,
            2..2
        );
        assert_eq!(
            state
                .previous(text, revision(0), r"\b", true, true)
                .unwrap()
                .byte_range,
            0..0
        );
        assert_eq!(
            state
                .previous(text, revision(0), r"\b", true, true)
                .unwrap()
                .byte_range,
            4..4
        );
    }

    #[test]
    fn empty_replacement_of_zero_width_regex_advances_and_wraps() {
        let mut state = SearchSession::default();
        let mut text = "é x".to_owned();
        assert_eq!(
            state
                .next(&text, revision(0), r"\b", true, true)
                .unwrap()
                .byte_range,
            0..0
        );

        for expected in [2..2, 3..3, 4..4, 0..0] {
            assert!(state.replace_one(&mut text, revision(0), r"\b", "", true, true));
            assert_eq!(text, "é x");
            assert_eq!(state.selected().unwrap().byte_range, expected);
        }
    }

    #[test]
    fn regex_rejects_pathological_nonmatches_without_recursive_backtracking() {
        let text = "a".repeat(2000);
        assert!(find_all_with_options(&text, "a*a*a*a*a*a*a*a*b", true, true).is_empty());
    }

    #[test]
    fn literal_find_does_not_interpret_regex_characters() {
        assert_eq!(
            find_next_with_options("a.*b aZZb", "a.*b", None, true, false)
                .unwrap()
                .byte_range,
            0..4
        );
        let matches = find_all_with_options("a.*b aZZb a.*b", "a.*b", true, false);
        assert_eq!(byte_ranges(&matches), vec![0..4, 10..14]);
    }

    #[test]
    fn matches_report_utf8_byte_and_char_ranges() {
        let text = "á 🦀café 🦀";
        let crabs = find_all_with_options(text, "🦀", true, false);
        assert_eq!(byte_ranges(&crabs), vec![4..8, 14..18]);
        assert_eq!(char_ranges(&crabs), vec![3..4, 9..10]);

        let cafe = find_all_with_options(text, "café", true, false);
        assert_eq!(byte_ranges(&cafe), vec![8..13]);
        assert_eq!(char_ranges(&cafe), vec![4..8]);
    }

    #[test]
    fn constructing_ranges_rejects_invalid_utf8_boundaries() {
        let text = "a🦀b";
        assert!(SearchMatch::from_byte_range(text, 1..5).is_some());
        assert!(SearchMatch::from_byte_range(text, 2..5).is_none());
        assert!(SearchMatch::from_byte_range(text, 1..4).is_none());
        assert!(SearchMatch::from_byte_range(text, 6..7).is_none());
        let reversed = Range { start: 5, end: 4 };
        assert!(SearchMatch::from_byte_range(text, reversed).is_none());
    }

    #[test]
    fn next_and_previous_wrap() {
        let text = "one two one";
        let first = find_next_with_options(text, "one", None, true, false).unwrap();
        assert_eq!(first.byte_range, 0..3);
        let second = find_next_with_options(text, "one", Some(&first), true, false).unwrap();
        assert_eq!(second.byte_range, 8..11);
        assert_eq!(
            find_next_with_options(text, "one", Some(&second), true, false).unwrap(),
            first
        );

        let last = find_previous_with_options(text, "one", None, true, false).unwrap();
        assert_eq!(last.byte_range, 8..11);
        assert_eq!(
            find_previous_with_options(text, "one", Some(&last), true, false).unwrap(),
            first
        );
        assert_eq!(
            find_previous_with_options(text, "one", Some(&first), true, false).unwrap(),
            last
        );
    }

    #[test]
    fn stale_selections_are_ignored() {
        let stale = SearchMatch {
            byte_range: 0..3,
            char_range: 0..3,
        };
        assert_eq!(
            find_next_with_options("zero one", "one", Some(&stale), true, false)
                .unwrap()
                .byte_range,
            5..8
        );
        assert_eq!(
            find_previous_with_options("zero one", "one", Some(&stale), true, false)
                .unwrap()
                .byte_range,
            5..8
        );
    }

    #[test]
    fn empty_query_never_matches_or_replaces() {
        assert!(find_all_with_options("abc", "", true, false).is_empty());
        assert!(find_next_with_options("abc", "", None, true, false).is_none());
        assert!(find_previous_with_options("abc", "", None, true, false).is_none());

        let mut text = "abc".to_owned();
        assert_eq!(
            SearchSession::default().replace_all(&mut text, revision(0), "", "x", true, false,),
            0
        );
        assert_eq!(text, "abc");
    }

    #[test]
    fn replace_one_uses_utf8_safe_offsets_and_selects_the_next_match() {
        let mut text = "🦀 and 🦀 and 🦀".to_owned();
        let mut search = SearchSession::default();
        assert_eq!(
            search
                .next(&text, revision(0), "🦀", true, false)
                .unwrap()
                .char_range,
            0..1
        );

        assert!(search.replace_one(&mut text, revision(0), "🦀", "café", true, false,));
        assert_eq!(text, "café and 🦀 and 🦀");
        assert_eq!(search.selected().unwrap().char_range, 9..10);

        assert!(search.replace_one(&mut text, revision(1), "🦀", "x", true, false,));
        assert_eq!(text, "café and x and 🦀");
        assert_eq!(search.selected().unwrap().char_range, 15..16);
    }

    #[test]
    fn replace_one_selects_and_replaces_when_no_match_was_selected() {
        let mut text = "one two one".to_owned();
        let mut search = SearchSession::default();
        assert!(search.replace_one(&mut text, revision(0), "one", "1", true, false,));
        assert_eq!(text, "1 two one");
        assert_eq!(search.selected().unwrap().byte_range, 6..9);
    }

    #[test]
    fn replace_one_does_not_immediately_select_query_inside_replacement() {
        let mut text = "a then a".to_owned();
        let mut search = SearchSession::default();
        search.next(&text, revision(0), "a", true, false);
        assert!(search.replace_one(&mut text, revision(0), "a", "aa", true, false,));
        assert_eq!(text, "aa then a");
        assert_eq!(search.selected().unwrap().byte_range, 8..9);
    }

    #[test]
    fn replace_all_terminates_when_replacement_contains_query() {
        let mut text = "a banana".to_owned();
        assert_eq!(
            SearchSession::default().replace_all(&mut text, revision(0), "a", "aa", true, false,),
            4
        );
        assert_eq!(text, "aa baanaanaa");
    }

    #[test]
    fn replace_all_uses_original_non_overlapping_matches() {
        let mut text = "aaaa".to_owned();
        assert_eq!(
            SearchSession::default().replace_all(&mut text, revision(0), "aa", "aaa", true, false,),
            2
        );
        assert_eq!(text, "aaaaaa");

        let mut unicode = "🙂🙂 x 🙂🙂".to_owned();
        assert_eq!(
            SearchSession::default().replace_all(
                &mut unicode,
                revision(0),
                "🙂🙂",
                "🙂",
                true,
                false
            ),
            2
        );
        assert_eq!(unicode, "🙂 x 🙂");
    }

    #[test]
    fn state_clears_after_replace_all_and_on_empty_replace_one() {
        let mut text = "x x".to_owned();
        let mut state = SearchSession::default();
        state.next(&text, revision(0), "x", true, false);
        assert_eq!(
            state.replace_all(&mut text, revision(0), "x", "y", true, false),
            2
        );
        assert!(state.selected().is_none());

        state.next(&text, revision(1), "y", true, false);
        assert!(!state.replace_one(&mut text, revision(1), "", "z", true, false,));
        assert!(state.selected().is_none());
    }

    #[test]
    fn replace_all_preserves_unicode_and_uses_literal_replacements_in_every_mode() {
        for (original, query, replacement, case_sensitive, regex, count, expected) in [
            ("é é", "é", "🙂", true, false, 2, "🙂 🙂"),
            ("É é", "é", "🙂", false, false, 2, "🙂 🙂"),
            ("İ i\u{307}", "i\u{307}", "x", false, false, 2, "x x"),
            ("a1 a2", r"a(\d)", "$1", true, true, 2, "$1 $1"),
            ("É é", "é", "🙂", false, true, 2, "🙂 🙂"),
            ("é x", r"\b", "_", true, true, 4, "_é_ _x_"),
        ] {
            let mut text = original.to_owned();
            let mut state = SearchSession::default();
            state.next(&text, revision(0), query, case_sensitive, regex);
            assert_eq!(
                state.replace_all(
                    &mut text,
                    revision(0),
                    query,
                    replacement,
                    case_sensitive,
                    regex,
                ),
                count
            );
            assert_eq!(text, expected);
            assert!(state.selected().is_none());
        }
    }

    #[test]
    fn replace_all_without_matches_clears_stale_selection_in_every_mode() {
        for case_sensitive in [false, true] {
            for regex in [false, true] {
                for query in ["", "missing"] {
                    let mut text = "x".to_owned();
                    let mut state = SearchSession::default();
                    state.next(&text, revision(0), "x", case_sensitive, regex);
                    assert_eq!(
                        state.replace_all(
                            &mut text,
                            revision(0),
                            query,
                            "y",
                            case_sensitive,
                            regex,
                        ),
                        0
                    );
                    assert_eq!(text, "x");
                    assert!(state.selected().is_none());
                }
            }
        }
    }

    #[test]
    fn replace_one_literal_search_starts_at_the_end_of_the_replacement() {
        let mut state = SearchSession::default();
        let mut text = "aaaa".to_owned();
        assert!(state.replace_one(&mut text, revision(0), "aa", "a", true, false,));
        assert_eq!(text, "aaa");
        assert_eq!(state.selected().unwrap().byte_range, 1..3);

        // Navigation and replacement consume the same cached, non-overlapping
        // result set after the edit.
        state.clear();
        text = "aaa".to_owned();
        assert!(state.replace_one(&mut text, revision(1), "aa", "a", true, false,));
        assert_eq!(text, "aa");
        assert!(state.selected().is_none());
    }

    #[test]
    fn replace_one_consumes_an_anchored_overlapping_selection() {
        let mut state = SearchSession::default();
        let mut text = "aaaa".to_owned();
        assert!(state.replace_one(&mut text, revision(0), "aa", "a", true, false));
        assert_eq!(state.selected().unwrap().byte_range, 1..3);

        assert!(state.replace_one(&mut text, revision(1), "aa", "a", true, false));
        assert_eq!(text, "aa");
        assert_eq!(state.selected().unwrap().byte_range, 0..2);
    }

    #[test]
    fn navigation_handles_adjacent_unicode_matches_without_losing_char_offsets() {
        let text = "éé x éé";
        let first = find_next_with_options(text, "é", None, true, false).unwrap();
        let second = find_next_with_options(text, "é", Some(&first), true, false).unwrap();
        let third = find_next_with_options(text, "é", Some(&second), true, false).unwrap();
        let fourth = find_next_with_options(text, "é", Some(&third), true, false).unwrap();

        assert_eq!(first.char_range, 0..1);
        assert_eq!(second.char_range, 1..2);
        assert_eq!(third.char_range, 5..6);
        assert_eq!(fourth.char_range, 6..7);
        assert_eq!(
            find_next_with_options(text, "é", Some(&fourth), true, false).unwrap(),
            first
        );
        assert_eq!(
            find_previous_with_options(text, "é", Some(&first), true, false).unwrap(),
            fourth
        );
    }

    #[test]
    fn replacing_many_matches_preserves_original_non_overlapping_semantics() {
        let mut text = "ab".repeat(10_000);
        assert_eq!(
            SearchSession::default().replace_all(&mut text, revision(0), "ab", "xyz", true, false,),
            10_000
        );
        assert_eq!(text.len(), 30_000);
        assert!(text.starts_with("xyzxyz"));
        assert!(text.ends_with("xyzxyz"));
    }
}
