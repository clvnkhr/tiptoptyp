//! Local filtering and safe rebasing of completion edits while typing.
use crate::{
    font_catalog::FontCatalog,
    lsp_text::{lsp_position_at_char, range_to_char_range},
    tinymist::{CompletionItem, LspRange, LspTextEdit},
};
use std::ops::Range;
use typst_syntax::{LinkedNode, Source, SyntaxKind};

/// Case-insensitive subsequence matching. Earlier and consecutive matches rank first.
pub(crate) fn fuzzy_score(candidate: &str, query: &str) -> Option<usize> {
    let candidate = candidate.to_lowercase();
    let mut remaining = candidate.chars().enumerate();
    let mut score = 0;
    let mut previous = None;
    for character in query.to_lowercase().chars() {
        let (index, _) = remaining.find(|(_, value)| *value == character)?;
        score += index
            + if previous.is_some_and(|previous| index == previous + 1) {
                0
            } else {
                8
            };
        previous = Some(index);
    }
    Some(score)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FontContext {
    pub(crate) range: Range<usize>,
    pub(crate) query: String,
    quoted: bool,
}

pub(crate) fn font_context(source: &str, cursor: usize) -> Option<FontContext> {
    let byte = source
        .char_indices()
        .nth(cursor)
        .map_or(source.len(), |(byte, _)| byte);
    let prefix = &source[..byte];
    // Complete an unfinished literal before asking the syntax tree to confirm
    // it is the font argument of text(), excluding comments and other calls.
    let colon = prefix.rfind(':')?;
    let value = prefix[colon + 1..].trim_start();
    let quoted = value.starts_with('"');
    if (!quoted && !value.is_empty())
        || value
            .get(1..)
            .is_some_and(|value| value.contains(['"', '\\', '\n']))
    {
        return None;
    }
    let start_byte = byte - value.len() + usize::from(quoted);
    let query = source[start_byte..byte].to_owned();
    let probe = format!(
        "{}\"tiptoptyp-font-probe\")",
        &source[..start_byte - usize::from(quoted)]
    );
    let parsed = Source::detached(probe);
    fn is_font(node: LinkedNode<'_>, byte: usize) -> bool {
        if matches!(node.kind(), SyntaxKind::SetRule | SyntaxKind::FuncCall) {
            let text = node
                .children()
                .find(|child| child.kind() == SyntaxKind::Ident)
                .is_some_and(|name| name.leaf_text() == "text");
            if text
                && let Some(args) = node
                    .children()
                    .find(|child| child.kind() == SyntaxKind::Args)
            {
                for named in args
                    .children()
                    .filter(|child| child.kind() == SyntaxKind::Named)
                {
                    if named
                        .children()
                        .find(|child| child.kind() == SyntaxKind::Ident)
                        .is_some_and(|name| name.leaf_text() == "font")
                        && named.range().contains(&byte)
                    {
                        return true;
                    }
                }
            }
        }
        node.children().any(|child| is_font(child, byte))
    }
    if !is_font(LinkedNode::new(parsed.root()), start_byte) {
        return None;
    }
    let start = source[..start_byte].chars().count();
    let suffix = &source[byte..];
    let end = if quoted {
        cursor
            + suffix
                .chars()
                .take_while(|c| !matches!(c, '"' | '\n' | '\r'))
                .count()
    } else {
        cursor
    };
    // If no closing quote exists, stop at the caret instead of swallowing
    // the rest of an incomplete document.
    let end = if quoted && suffix.chars().take_while(|c| *c != '\n').any(|c| c == '"') {
        end
    } else {
        cursor
    };
    Some(FontContext {
        range: start..end,
        query,
        quoted,
    })
}

pub(crate) fn font_items(
    source: &str,
    cursor: usize,
    catalog: &FontCatalog,
) -> Option<Vec<CompletionItem>> {
    let context = font_context(source, cursor)?;
    let suffix_quote = source.chars().nth(context.range.end) == Some('"');
    Some(
        catalog
            .document_family_names()
            .into_iter()
            .map(|name| {
                let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
                let inserted = format!(
                    "{}{escaped}{}",
                    if context.quoted { "" } else { "\"" },
                    if suffix_quote { "" } else { "\"" }
                );
                CompletionItem {
                    label: name.to_owned(),
                    detail: Some("Font".into()),
                    documentation: None,
                    filter_text: Some(name.to_owned()),
                    sort_text: None,
                    insert_text: inserted.clone(),
                    insert_text_is_snippet: false,
                    text_edit: Some(LspTextEdit {
                        range: LspRange {
                            start: lsp_position_at_char(source, context.range.start),
                            end: lsp_position_at_char(source, context.range.end),
                        },
                        new_text: inserted,
                    }),
                    additional_text_edits: Vec::new(),
                }
            })
            .collect(),
    )
}

pub(crate) fn query(source: &str, cursor: usize) -> String {
    if let Some(font) = font_context(source, cursor) {
        return font.query;
    }
    if let Some(start) = reference_start(source, cursor) {
        return source
            .chars()
            .skip(start + 1)
            .take(cursor - start - 1)
            .collect();
    }
    let prefix: String = source.chars().take(cursor).collect();
    prefix
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn reference_start(source: &str, cursor: usize) -> Option<usize> {
    let prefix: Vec<_> = source.chars().take(cursor).collect();
    let length = prefix
        .iter()
        .rev()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
        .count();
    let start = cursor.checked_sub(length + 1)?;
    matches!(source.chars().nth(start), Some('@' | '<')).then_some(start)
}

fn reference_code(item: &CompletionItem, source: &str, cursor: usize) -> Option<String> {
    let start = reference_start(source, cursor)?;
    let text = item
        .text_edit
        .as_ref()
        .map_or(item.insert_text.as_str(), |edit| edit.new_text.as_str());
    let edit_start = item.text_edit.as_ref().map_or(start + 1, |edit| {
        range_to_char_range(source, &edit.range).start
    });
    let before: String = source
        .chars()
        .skip(start)
        .take(edit_start.saturating_sub(start))
        .collect();
    Some(format!("{before}{text}"))
}

pub(crate) fn display_label(item: &CompletionItem, source: &str, cursor: usize) -> String {
    let code = reference_code(item, source, cursor);
    let primary = code.as_deref().unwrap_or(&item.label);
    let mut parts = vec![primary.to_owned()];
    if code.is_some() && primary != item.label {
        parts.push(item.label.clone());
    }
    if let Some(detail) = &item.detail {
        parts.push(detail.clone());
    }
    parts.join("  —  ")
}

pub(crate) fn filtered_for_source(
    items: &[CompletionItem],
    source: &str,
    cursor: usize,
) -> Vec<CompletionItem> {
    if reference_start(source, cursor).is_none() {
        return filtered(items, &query(source, cursor));
    }
    let candidates = items
        .iter()
        .map(|item| {
            let mut item = item.clone();
            item.filter_text = reference_code(&item, source, cursor);
            item
        })
        .collect::<Vec<_>>();
    filtered(&candidates, &query(source, cursor))
}

pub(crate) fn filtered(items: &[CompletionItem], query: &str) -> Vec<CompletionItem> {
    let mut matches = items
        .iter()
        .filter_map(|item| {
            fuzzy_score(
                item.filter_text
                    .as_deref()
                    .unwrap_or(&item.label)
                    .trim_start_matches(['@', '<'])
                    .trim_matches('"'),
                query,
            )
            .map(|score| (score, item))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(ls, left), (rs, right)| {
        ls.cmp(rs).then_with(|| {
            left.sort_text
                .as_deref()
                .unwrap_or(&left.label)
                .cmp(right.sort_text.as_deref().unwrap_or(&right.label))
        })
    });
    matches
        .into_iter()
        .take(200)
        .map(|(_, item)| item.clone())
        .collect()
}

/// Only edits to the prefix immediately before the caret may retain results.
/// Main and additional LSP edits are rebased in Unicode scalar coordinates.
pub(crate) fn rebase(
    items: &[CompletionItem],
    old: &str,
    old_cursor: usize,
    new: &str,
    cursor: usize,
) -> Option<Vec<CompletionItem>> {
    let old_chars = old.chars().collect::<Vec<_>>();
    let new_chars = new.chars().collect::<Vec<_>>();
    if old_cursor > old_chars.len()
        || cursor > new_chars.len()
        || old_chars[old_cursor..] != new_chars[cursor..]
    {
        return None;
    }
    let common = old_chars[..old_cursor]
        .iter()
        .zip(&new_chars[..cursor])
        .take_while(|(a, b)| a == b)
        .count();
    let is_reference =
        reference_start(old, old_cursor).is_some() && reference_start(new, cursor).is_some();
    if old_chars[common..old_cursor]
        .iter()
        .chain(&new_chars[common..cursor])
        .any(|c| {
            !(c.is_alphanumeric()
                || matches!(c, '_' | '-')
                || is_reference && matches!(c, ':' | '.'))
        })
    {
        return None;
    }
    let map = |offset: usize| {
        if offset <= common {
            offset
        } else if offset >= old_cursor {
            cursor + offset - old_cursor
        } else {
            cursor
        }
    };
    let mut result = Vec::new();
    for item in items {
        let mut item = item.clone();
        let mut edits = item.additional_text_edits.clone();
        edits.extend(item.text_edit.clone());
        if crate::lsp_text::apply_text_edits(old, &edits, [old_cursor, old_cursor]).is_err() {
            continue;
        }
        if let Some(edit) = &mut item.text_edit {
            let range = range_to_char_range(old, &edit.range);
            if range.start > common || range.end < old_cursor {
                continue;
            }
            edit.range = LspRange {
                start: lsp_position_at_char(new, range.start),
                end: lsp_position_at_char(new, cursor + range.end - old_cursor),
            };
        }
        let mut valid = true;
        for edit in &mut item.additional_text_edits {
            let range = range_to_char_range(old, &edit.range);
            if range.start < old_cursor && range.end > common {
                valid = false;
                break;
            }
            edit.range = LspRange {
                start: lsp_position_at_char(new, map(range.start)),
                end: lsp_position_at_char(new, map(range.end)),
            };
        }
        if valid {
            result.push(item);
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_queries_exclude_the_receiver_but_references_keep_code_punctuation() {
        for (source, expected) in [
            ("#calc.ab", "ab"),
            ("#text(fill: re", "re"),
            ("@sec.intro:one", "sec.intro:one"),
        ] {
            assert_eq!(query(source, source.chars().count()), expected);
        }
        assert!(rebase(&[], "#calc", 5, "#calc.", 6).is_none());
        assert!(rebase(&[], "@sec", 4, "@sec.", 5).is_some());
    }

    fn item(source: &str, range: Range<usize>, insert: &str) -> CompletionItem {
        CompletionItem {
            label: "Human-readable title".into(),
            detail: Some("Section 1".into()),
            documentation: None,
            filter_text: None,
            sort_text: None,
            insert_text: insert.into(),
            insert_text_is_snippet: false,
            text_edit: Some(LspTextEdit {
                range: LspRange {
                    start: lsp_position_at_char(source, range.start),
                    end: lsp_position_at_char(source, range.end),
                },
                new_text: insert.into(),
            }),
            additional_text_edits: Vec::new(),
        }
    }

    #[test]
    fn reference_rows_start_with_code_and_filter_by_code_instead_of_title() {
        let source = "See @in";
        let item = item(source, 5..7, "intro");
        assert_eq!(
            display_label(&item, source, 7),
            "@intro  —  Human-readable title  —  Section 1"
        );
        assert_eq!(filtered_for_source(&[item], source, 7).len(), 1);
    }

    #[test]
    fn typing_and_backspace_rebase_unicode_main_and_additional_edits() {
        let old = "🦀 #he\nend";
        let mut item = item(old, 3..5, "heading");
        item.additional_text_edits.push(LspTextEdit {
            range: LspRange {
                start: lsp_position_at_char(old, 6),
                end: lsp_position_at_char(old, 9),
            },
            new_text: "tail".into(),
        });
        let new = "🦀 #hea\nend";
        let rebased = rebase(&[item.clone()], old, 5, new, 6).unwrap();
        assert_eq!(
            crate::lsp_text::apply_text_edits(
                new,
                &[
                    rebased[0].text_edit.clone().unwrap(),
                    rebased[0].additional_text_edits[0].clone()
                ],
                [0, 0]
            )
            .unwrap()
            .text,
            "🦀 #heading\ntail"
        );
        let restored = rebase(&rebased, new, 6, old, 5).unwrap();
        assert_eq!(restored, [item]);
        assert!(rebase(&rebased, new, 6, "🦀 #hea elsewhere", 6).is_none());
    }

    #[test]
    fn filtering_happens_before_the_visible_limit() {
        let items = (0..500)
            .map(|i| {
                let mut item = item("#x", 1..2, "x");
                item.label = format!("name{i:03}");
                item
            })
            .collect::<Vec<_>>();
        assert_eq!(filtered(&items, "499")[0].label, "name499");
    }

    #[test]
    fn fuzzy_matching_handles_gaps_case_and_unicode() {
        assert!(fuzzy_score("New Computer Modern", "ncm").is_some());
        assert!(fuzzy_score("Résumé", "RéS").is_some());
        assert!(fuzzy_score("New Computer Modern", "xyz").is_none());
        assert!(fuzzy_score("abcd", "ab") < fuzzy_score("axby", "ab"));
    }
    #[test]
    fn fonts_are_offered_after_colon_space_and_open_quote_only_in_text_calls() {
        for source in [
            "#set text(font:",
            "#set text(font: ",
            "#set text(font: \"",
            "#text(font: \"New Co",
        ] {
            assert!(
                font_context(source, source.chars().count()).is_some(),
                "{source}"
            );
        }
        for source in [
            "// #set text(font: \"",
            "#other(font: \"",
            "#set text(size: \"",
            "#set text(font: family",
        ] {
            assert!(
                font_context(source, source.chars().count()).is_none(),
                "{source}"
            );
        }
    }
    #[test]
    fn font_edits_preserve_closing_quotes_and_filter_before_limiting() {
        let source = "#set text(font: \"New Co\")";
        let cursor = source.find("Co").unwrap() + 2;
        let items = font_items(source, cursor, &FontCatalog::snapshot_fixture()).unwrap();
        let selected = filtered(&items, "ncm");
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected[0].text_edit.as_ref().unwrap().new_text,
            "New Computer Modern"
        );
    }
}
