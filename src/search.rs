use std::ops::Range;

/// A literal match expressed in both of the coordinate systems used by the
/// editor: UTF-8 byte offsets for editing `String`s and Unicode scalar offsets
/// for positioning an egui text cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub byte_range: Range<usize>,
    pub char_range: Range<usize>,
}

#[allow(dead_code)]
impl SearchMatch {
    /// Build a match from a byte range, rejecting ranges which are out of
    /// bounds, reversed, or not on UTF-8 character boundaries.
    pub fn from_byte_range(text: &str, byte_range: Range<usize>) -> Option<Self> {
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

    /// Return the matched text when this range still describes `text`.
    pub fn as_str<'a>(&self, text: &'a str) -> Option<&'a str> {
        text.get(self.byte_range.clone())
    }
}

/// Find the first case-sensitive literal match.
pub fn find(text: &str, query: &str) -> Option<SearchMatch> {
    if query.is_empty() {
        return None;
    }

    let start = text.find(query)?;
    SearchMatch::from_byte_range(text, start..start + query.len())
}

/// Find the first literal match with an optional case-insensitive comparison.
#[allow(dead_code)]
pub fn find_with_case(text: &str, query: &str, case_sensitive: bool) -> Option<SearchMatch> {
    find_all_with_case(text, query, case_sensitive)
        .into_iter()
        .next()
}

/// Find all non-overlapping, case-sensitive literal matches.
///
/// Empty queries deliberately have no matches. This matches editor find
/// behavior and avoids treating every character boundary as a result.
pub fn find_all(text: &str, query: &str) -> Vec<SearchMatch> {
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

/// Find all non-overlapping literal matches while preserving UTF-8-safe
/// ranges. Folding is performed per Unicode scalar, and a match always spans
/// whole source characters.
pub fn find_all_with_case(text: &str, query: &str, case_sensitive: bool) -> Vec<SearchMatch> {
    if case_sensitive {
        return find_all(text, query);
    }
    let folded_query = folded(query);
    if folded_query.is_empty() {
        return Vec::new();
    }
    let chars = text.char_indices().collect::<Vec<_>>();
    let mut matches = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let mut candidate = String::new();
        let mut found = None;
        for (end, (_, character)) in chars.iter().enumerate().skip(index) {
            candidate.extend(character.to_lowercase());
            if candidate == folded_query {
                found = Some(end);
                break;
            }
            if candidate.len() >= folded_query.len() {
                break;
            }
        }
        if let Some(end) = found {
            let byte_start = chars[index].0;
            let byte_end = chars.get(end + 1).map_or(text.len(), |(byte, _)| *byte);
            matches.push(SearchMatch {
                byte_range: byte_start..byte_end,
                char_range: index..end + 1,
            });
            index = end + 1;
        } else {
            index += 1;
        }
    }
    matches
}

/// Find UTF-8-safe matches using the bounded, non-backtracking engine already
/// used by Syntect. Replacement text is literal (no capture expansion).
pub fn find_all_with_options(
    text: &str,
    query: &str,
    case_sensitive: bool,
    regex: bool,
) -> Vec<SearchMatch> {
    if !regex {
        return find_all_with_case(text, query, case_sensitive);
    }
    if query.is_empty() {
        return Vec::new();
    }
    let Ok(pattern) = regex_automata::meta::Regex::builder()
        .syntax(regex_automata::util::syntax::Config::new().case_insensitive(!case_sensitive))
        .build(query)
    else {
        return Vec::new();
    };
    // Convert sorted byte ranges in one pass, including zero-width matches.
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

fn folded(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// Find the match after `selected`, wrapping to the first match at the end.
/// A stale or unrelated selection is ignored.
#[allow(dead_code)]
pub fn find_next(text: &str, query: &str, selected: Option<&SearchMatch>) -> Option<SearchMatch> {
    find_next_with_case(text, query, selected, true)
}

#[allow(dead_code)]
pub fn find_next_with_case(
    text: &str,
    query: &str,
    selected: Option<&SearchMatch>,
    case_sensitive: bool,
) -> Option<SearchMatch> {
    find_next_with_options(text, query, selected, case_sensitive, false)
}

pub fn find_next_with_options(
    text: &str,
    query: &str,
    selected: Option<&SearchMatch>,
    case_sensitive: bool,
    regex: bool,
) -> Option<SearchMatch> {
    if query.is_empty() {
        return None;
    }

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
        .or_else(|| matches.into_iter().next())
}

/// Find the match before `selected`, wrapping to the final match at the start.
/// A stale or unrelated selection starts at the final match.
#[allow(dead_code)]
pub fn find_previous(
    text: &str,
    query: &str,
    selected: Option<&SearchMatch>,
) -> Option<SearchMatch> {
    find_previous_with_case(text, query, selected, true)
}

#[allow(dead_code)]
pub fn find_previous_with_case(
    text: &str,
    query: &str,
    selected: Option<&SearchMatch>,
    case_sensitive: bool,
) -> Option<SearchMatch> {
    find_previous_with_options(text, query, selected, case_sensitive, false)
}

pub fn find_previous_with_options(
    text: &str,
    query: &str,
    selected: Option<&SearchMatch>,
    case_sensitive: bool,
    regex: bool,
) -> Option<SearchMatch> {
    if query.is_empty() {
        return None;
    }

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
        .or_else(|| matches.into_iter().last())
}

/// Replace every non-overlapping literal match and return the number replaced.
///
/// Matches are collected from the original text and the result is assembled in
/// one pass. Consequently this always terminates even when `replacement`
/// contains `query` (for example, replacing `a` with `aa`).
pub fn replace_all(text: &mut String, query: &str, replacement: &str) -> usize {
    if query.is_empty() {
        return 0;
    }

    let mut matches = text.match_indices(query);
    let Some(first) = matches.next() else {
        return 0;
    };

    let mut result = String::with_capacity(text.len());
    let mut copied_until = 0;
    let mut count = 0;
    for (start, matched) in std::iter::once(first).chain(matches) {
        result.push_str(&text[copied_until..start]);
        result.push_str(replacement);
        copied_until = start + matched.len();
        count += 1;
    }
    result.push_str(&text[copied_until..]);
    *text = result;
    count
}

/// Stateful selection for a find/replace panel. The query and replacement text
/// can remain UI-owned; passing a changed query automatically invalidates a
/// selection which no longer matches it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchState {
    selected: Option<SearchMatch>,
}

#[allow(dead_code)]
impl SearchState {
    pub fn selected(&self) -> Option<&SearchMatch> {
        self.selected.as_ref()
    }

    pub fn clear(&mut self) {
        self.selected = None;
    }

    pub fn next(&mut self, text: &str, query: &str) -> Option<&SearchMatch> {
        self.next_with_case(text, query, true)
    }

    pub fn next_with_case(
        &mut self,
        text: &str,
        query: &str,
        case_sensitive: bool,
    ) -> Option<&SearchMatch> {
        self.next_with_options(text, query, case_sensitive, false)
    }

    pub fn next_with_options(
        &mut self,
        text: &str,
        query: &str,
        case_sensitive: bool,
        regex: bool,
    ) -> Option<&SearchMatch> {
        self.selected =
            find_next_with_options(text, query, self.selected.as_ref(), case_sensitive, regex);
        self.selected.as_ref()
    }

    pub fn previous(&mut self, text: &str, query: &str) -> Option<&SearchMatch> {
        self.previous_with_case(text, query, true)
    }

    pub fn previous_with_case(
        &mut self,
        text: &str,
        query: &str,
        case_sensitive: bool,
    ) -> Option<&SearchMatch> {
        self.previous_with_options(text, query, case_sensitive, false)
    }

    pub fn previous_with_options(
        &mut self,
        text: &str,
        query: &str,
        case_sensitive: bool,
        regex: bool,
    ) -> Option<&SearchMatch> {
        self.selected =
            find_previous_with_options(text, query, self.selected.as_ref(), case_sensitive, regex);
        self.selected.as_ref()
    }

    /// Replace the selected match. When there is no valid selection, the first
    /// match is selected and replaced. The following match is then selected.
    pub fn replace_one(&mut self, text: &mut String, query: &str, replacement: &str) -> bool {
        self.replace_one_with_case(text, query, replacement, true)
    }

    pub fn replace_one_with_case(
        &mut self,
        text: &mut String,
        query: &str,
        replacement: &str,
        case_sensitive: bool,
    ) -> bool {
        self.replace_one_with_options(text, query, replacement, case_sensitive, false)
    }

    pub fn replace_one_with_options(
        &mut self,
        text: &mut String,
        query: &str,
        replacement: &str,
        case_sensitive: bool,
        regex: bool,
    ) -> bool {
        if query.is_empty() {
            self.clear();
            return false;
        }

        if !self.selected.as_ref().is_some_and(|selected| {
            find_all_with_options(text, query, case_sensitive, regex)
                .iter()
                .any(|matched| matched.byte_range == selected.byte_range)
        }) {
            self.selected = find_next_with_options(text, query, None, case_sensitive, regex);
        }

        let Some(selected) = self.selected.take() else {
            return false;
        };
        let next_search_byte = selected.byte_range.start + replacement.len();
        text.replace_range(selected.byte_range, replacement);

        // Search after the inserted text so a replacement containing the query
        // does not immediately reselect itself. `find_next` supplies wrapping.
        let anchor = SearchMatch::from_byte_range(text, next_search_byte..next_search_byte);
        self.selected = find_next_after_anchor_with_options(
            text,
            query,
            anchor.as_ref(),
            case_sensitive,
            regex,
        );
        true
    }

    pub fn replace_all(&mut self, text: &mut String, query: &str, replacement: &str) -> usize {
        let count = replace_all(text, query, replacement);
        self.clear();
        count
    }

    pub fn replace_all_with_case(
        &mut self,
        text: &mut String,
        query: &str,
        replacement: &str,
        case_sensitive: bool,
    ) -> usize {
        self.replace_all_with_options(text, query, replacement, case_sensitive, false)
    }

    pub fn replace_all_with_options(
        &mut self,
        text: &mut String,
        query: &str,
        replacement: &str,
        case_sensitive: bool,
        regex: bool,
    ) -> usize {
        if regex {
            let matches = find_all_with_options(text, query, case_sensitive, true);
            if matches.is_empty() {
                return 0;
            }
            let mut result = String::with_capacity(text.len());
            let mut copied_until = 0;
            for matched in &matches {
                result.push_str(&text[copied_until..matched.byte_range.start]);
                result.push_str(replacement);
                copied_until = matched.byte_range.end;
            }
            result.push_str(&text[copied_until..]);
            *text = result;
            self.clear();
            return matches.len();
        }
        if case_sensitive {
            return self.replace_all(text, query, replacement);
        }
        let matches = find_all_with_case(text, query, false);
        if matches.is_empty() {
            return 0;
        }
        let mut result = String::with_capacity(text.len());
        let mut copied_until = 0;
        for matched in &matches {
            result.push_str(&text[copied_until..matched.byte_range.start]);
            result.push_str(replacement);
            copied_until = matched.byte_range.end;
        }
        result.push_str(&text[copied_until..]);
        *text = result;
        self.clear();
        matches.len()
    }
}

fn find_next_after_anchor(
    text: &str,
    query: &str,
    anchor: Option<&SearchMatch>,
) -> Option<SearchMatch> {
    if query.is_empty() {
        return None;
    }

    let start = anchor.map_or(0, |anchor| anchor.byte_range.end);
    find_at_or_after(text, query, start)
        .or_else(|| text.get(..start).and_then(|prefix| find(prefix, query)))
}

fn find_next_after_anchor_with_case(
    text: &str,
    query: &str,
    anchor: Option<&SearchMatch>,
    case_sensitive: bool,
) -> Option<SearchMatch> {
    if case_sensitive {
        return find_next_after_anchor(text, query, anchor);
    }
    let start = anchor.map_or(0, |anchor| anchor.byte_range.end);
    find_all_with_case(text, query, false)
        .into_iter()
        .find(|matched| matched.byte_range.start >= start)
        .or_else(|| {
            find_all_with_case(text, query, false)
                .into_iter()
                .find(|matched| matched.byte_range.start < start)
        })
}

fn find_next_after_anchor_with_options(
    text: &str,
    query: &str,
    anchor: Option<&SearchMatch>,
    case_sensitive: bool,
    regex: bool,
) -> Option<SearchMatch> {
    if !regex {
        return find_next_after_anchor_with_case(text, query, anchor, case_sensitive);
    }
    let matches = find_all_with_options(text, query, case_sensitive, true);
    let start = anchor.map_or(0, |anchor| anchor.byte_range.end);
    matches
        .iter()
        .find(|matched| matched.byte_range.start >= start)
        .cloned()
        .or_else(|| matches.into_iter().next())
}

fn find_at_or_after(text: &str, query: &str, start: usize) -> Option<SearchMatch> {
    let suffix = text.get(start..)?;
    let offset = suffix.find(query)?;
    let byte_start = start + offset;
    SearchMatch::from_byte_range(text, byte_start..byte_start + query.len())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let matches = find_all("Aa.a aa", "a");
        assert_eq!(byte_ranges(&matches), vec![1..2, 3..4, 5..6, 6..7]);

        let overlapping = find_all("aaaa", "aa");
        assert_eq!(byte_ranges(&overlapping), vec![0..2, 2..4]);
    }

    #[test]
    fn case_insensitive_find_preserves_unicode_ranges() {
        let matches = find_all_with_case("Alpha ALPHA 🦀", "alpha", false);
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
        assert!(find_all_with_options("abc", "[", true, true).is_empty());
        assert!(find_all_with_options("abc", "", true, true).is_empty());
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
        let mut state = SearchState::default();
        let text = "é x";
        assert_eq!(
            state
                .next_with_options(text, r"\b", true, true)
                .unwrap()
                .byte_range,
            0..0
        );
        assert_eq!(
            state
                .next_with_options(text, r"\b", true, true)
                .unwrap()
                .byte_range,
            2..2
        );
        assert_eq!(
            state
                .previous_with_options(text, r"\b", true, true)
                .unwrap()
                .byte_range,
            0..0
        );
        assert_eq!(
            state
                .previous_with_options(text, r"\b", true, true)
                .unwrap()
                .byte_range,
            4..4
        );
    }

    #[test]
    fn regex_rejects_pathological_nonmatches_without_recursive_backtracking() {
        let text = "a".repeat(2000);
        assert!(find_all_with_options(&text, "a*a*a*a*a*a*a*a*b", true, true).is_empty());
    }

    #[test]
    fn literal_find_does_not_interpret_regex_characters() {
        assert_eq!(find("a.*b aZZb", "a.*b").unwrap().byte_range, 0..4);
        let matches = find_all("a.*b aZZb a.*b", "a.*b");
        assert_eq!(byte_ranges(&matches), vec![0..4, 10..14]);
    }

    #[test]
    fn matches_report_utf8_byte_and_char_ranges() {
        let text = "á 🦀café 🦀";
        let crabs = find_all(text, "🦀");
        assert_eq!(byte_ranges(&crabs), vec![4..8, 14..18]);
        assert_eq!(char_ranges(&crabs), vec![3..4, 9..10]);

        let cafe = find_all(text, "café");
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
        let first = find_next(text, "one", None).unwrap();
        assert_eq!(first.byte_range, 0..3);
        let second = find_next(text, "one", Some(&first)).unwrap();
        assert_eq!(second.byte_range, 8..11);
        assert_eq!(find_next(text, "one", Some(&second)).unwrap(), first);

        let last = find_previous(text, "one", None).unwrap();
        assert_eq!(last.byte_range, 8..11);
        assert_eq!(find_previous(text, "one", Some(&last)).unwrap(), first);
        assert_eq!(find_previous(text, "one", Some(&first)).unwrap(), last);
    }

    #[test]
    fn stale_selections_are_ignored() {
        let stale = SearchMatch {
            byte_range: 0..3,
            char_range: 0..3,
        };
        assert_eq!(
            find_next("zero one", "one", Some(&stale))
                .unwrap()
                .byte_range,
            5..8
        );
        assert_eq!(
            find_previous("zero one", "one", Some(&stale))
                .unwrap()
                .byte_range,
            5..8
        );
    }

    #[test]
    fn empty_query_never_matches_or_replaces() {
        assert!(find("abc", "").is_none());
        assert!(find_all("abc", "").is_empty());
        assert!(find_next("abc", "", None).is_none());
        assert!(find_previous("abc", "", None).is_none());

        let mut text = "abc".to_owned();
        assert_eq!(replace_all(&mut text, "", "x"), 0);
        assert_eq!(text, "abc");
    }

    #[test]
    fn replace_one_uses_utf8_safe_offsets_and_selects_the_next_match() {
        let mut text = "🦀 and 🦀 and 🦀".to_owned();
        let mut search = SearchState::default();
        assert_eq!(search.next(&text, "🦀").unwrap().char_range, 0..1);

        assert!(search.replace_one(&mut text, "🦀", "café"));
        assert_eq!(text, "café and 🦀 and 🦀");
        assert_eq!(search.selected().unwrap().char_range, 9..10);

        assert!(search.replace_one(&mut text, "🦀", "x"));
        assert_eq!(text, "café and x and 🦀");
        assert_eq!(search.selected().unwrap().char_range, 15..16);
    }

    #[test]
    fn replace_one_selects_and_replaces_when_no_match_was_selected() {
        let mut text = "one two one".to_owned();
        let mut search = SearchState::default();
        assert!(search.replace_one(&mut text, "one", "1"));
        assert_eq!(text, "1 two one");
        assert_eq!(search.selected().unwrap().byte_range, 6..9);
    }

    #[test]
    fn replace_one_does_not_immediately_select_query_inside_replacement() {
        let mut text = "a then a".to_owned();
        let mut search = SearchState::default();
        search.next(&text, "a");
        assert!(search.replace_one(&mut text, "a", "aa"));
        assert_eq!(text, "aa then a");
        assert_eq!(search.selected().unwrap().byte_range, 8..9);
    }

    #[test]
    fn replace_all_terminates_when_replacement_contains_query() {
        let mut text = "a banana".to_owned();
        assert_eq!(replace_all(&mut text, "a", "aa"), 4);
        assert_eq!(text, "aa baanaanaa");
    }

    #[test]
    fn replace_all_uses_original_non_overlapping_matches() {
        let mut text = "aaaa".to_owned();
        assert_eq!(replace_all(&mut text, "aa", "aaa"), 2);
        assert_eq!(text, "aaaaaa");

        let mut unicode = "🙂🙂 x 🙂🙂".to_owned();
        assert_eq!(replace_all(&mut unicode, "🙂🙂", "🙂"), 2);
        assert_eq!(unicode, "🙂 x 🙂");
    }

    #[test]
    fn state_clears_after_replace_all_and_on_empty_replace_one() {
        let mut text = "x x".to_owned();
        let mut state = SearchState::default();
        state.next(&text, "x");
        assert_eq!(state.replace_all(&mut text, "x", "y"), 2);
        assert!(state.selected().is_none());

        state.next(&text, "y");
        assert!(!state.replace_one(&mut text, "", "z"));
        assert!(state.selected().is_none());
    }

    #[test]
    fn navigation_handles_adjacent_unicode_matches_without_losing_char_offsets() {
        let text = "éé x éé";
        let first = find_next(text, "é", None).unwrap();
        let second = find_next(text, "é", Some(&first)).unwrap();
        let third = find_next(text, "é", Some(&second)).unwrap();
        let fourth = find_next(text, "é", Some(&third)).unwrap();

        assert_eq!(first.char_range, 0..1);
        assert_eq!(second.char_range, 1..2);
        assert_eq!(third.char_range, 5..6);
        assert_eq!(fourth.char_range, 6..7);
        assert_eq!(find_next(text, "é", Some(&fourth)).unwrap(), first);
        assert_eq!(find_previous(text, "é", Some(&first)).unwrap(), fourth);
    }

    #[test]
    fn replacing_many_matches_preserves_original_non_overlapping_semantics() {
        let mut text = "ab".repeat(10_000);
        assert_eq!(replace_all(&mut text, "ab", "xyz"), 10_000);
        assert_eq!(text.len(), 30_000);
        assert!(text.starts_with("xyzxyz"));
        assert!(text.ends_with("xyzxyz"));
    }
}
