//! Stateless delimiter editing rules. Cursor positions use Unicode scalars.
//! No generated-pair ledger can go stale after undo, paste, or a window switch.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedAction {
    InsertPair(char),
    StepOver,
    Insert,
}

pub fn closer(open: char) -> Option<char> {
    Some(match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        '$' | '"' | '`' | '*' | '_' => open,
        _ => return None,
    })
}

pub fn typed_action(
    typed: char,
    next: Option<char>,
    may_open: bool,
    may_step_over: bool,
) -> TypedAction {
    if may_step_over && next == Some(typed) {
        return TypedAction::StepOver;
    }
    // Inserting into a word should not leave an unsolicited closer behind.
    if may_open
        && next.is_none_or(|next| next.is_whitespace() || ")]}>$\"`*_,;:".contains(next))
        && let Some(close) = closer(typed)
    {
        return TypedAction::InsertPair(close);
    }
    TypedAction::Insert
}

pub fn empty_pair(before: Option<char>, after: Option<char>) -> bool {
    before
        .and_then(closer)
        .is_some_and(|close| Some(close) == after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_opener_has_an_atomic_pair_and_closer_can_be_skipped() {
        for open in "([{<$\"`*_".chars() {
            let close = closer(open).unwrap();
            assert_eq!(
                typed_action(open, None, true, false),
                TypedAction::InsertPair(close)
            );
            assert!(empty_pair(Some(open), Some(close)));
            assert_eq!(
                typed_action(close, Some(close), true, true),
                TypedAction::StepOver
            );
        }
    }

    #[test]
    fn literal_contexts_and_words_are_not_paired_or_skipped() {
        assert_eq!(
            typed_action('(', Some('文'), true, false),
            TypedAction::Insert
        );
        assert_eq!(typed_action('(', None, false, false), TypedAction::Insert);
        assert_eq!(
            typed_action(')', Some(')'), false, false),
            TypedAction::Insert
        );
        assert!(!empty_pair(Some('('), Some(']')));
        assert!(!empty_pair(None, Some(')')));
    }
}
