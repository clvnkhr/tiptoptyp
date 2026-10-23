//! Visible, editable defaults for mathematical prose. These are initial JSON
//! values, never hidden filters or additions to an existing user configuration.
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) const QUIET_RULES: [&str; 11] = [
    "WHITESPACE_RULE",
    "WHITESPACE_PARAGRAPH_BEGIN",
    "WHITESPACE_PARAGRAPH",
    "EMPTY_LINE",
    "TOO_LONG_SENTENCE",
    "TOO_LONG_PARAGRAPH",
    "PARAGRAPH_REPEAT_BEGINNING_RULE",
    "ENGLISH_WORD_REPEAT_BEGINNING_RULE",
    "EN_PLAIN_ENGLISH_REPLACE",
    "EN_REDUNDANCY_REPLACE",
    "EN_DASH_RULE",
];

pub(super) fn default_configuration() -> Value {
    // Reading the compiled word list does not construct Harper's FST or lint
    // engine. Seed both dialects explicitly so every accepted word is editable.
    let mut words = BTreeSet::new();
    for word in include_str!("../../assets/dictionaries/mathematics.txt").lines() {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        words.insert(word.to_owned());
        if word.contains('-') {
            words.insert(word.replace('-', "\u{2010}"));
            words.insert(word.replace('-', "\u{2011}"));
        }
    }
    let words: Vec<_> = words.into_iter().collect();
    json!({
        "language": "en-GB",
        "checkFrequency": "manual",
        "completionEnabled": false,
        "additionalRules": {"enablePickyRules": false},
        "dictionary": {"en-GB": words, "en-US": words},
        "disabledRules": {"en-GB": QUIET_RULES, "en-US": QUIET_RULES},
        "enabledRules": {},
        "hiddenFalsePositives": {},
        "ltex-ls": {"logLevel": "warning"}
    })
}
