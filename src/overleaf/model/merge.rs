use std::{collections::HashMap, ops::Range};

const MAX_LINES: usize = 100_000;
const MAX_LINE_WORK: usize = 500_000;
const MAX_SCALARS: usize = 2_048;
const MAX_MATRIX: usize = 250_000;
const MAX_SCALAR_WORK: usize = 1_000_000;

#[derive(Debug, PartialEq, Eq)]
struct Edit {
    range: Range<usize>,
    text: String,
}
#[derive(Default)]
struct Budget {
    lines: usize,
    scalars: usize,
}

/// Conservative, bounded three-way merge. Patience line anchors isolate small
/// changes before scalar refinement; ambiguous or oversized overlaps require
/// review. Every range remains relative to the same authoritative base.
pub(super) fn reconcile(base: &str, local: &str, remote: &str) -> Option<String> {
    if local == remote || local == base {
        return Some(remote.into());
    }
    if remote == base {
        return Some(local.into());
    }
    let mut budget = Budget::default();
    let mut edits = changes(base, local, &mut budget)?;
    edits.extend(changes(base, remote, &mut budget)?);
    edits.sort_unstable_by_key(|edit| (edit.range.start, edit.range.end));
    edits.dedup();
    // Both inputs already contain disjoint base-relative edits. Once sorted,
    // any overlap must involve adjacent edits, keeping reconciliation linear.
    if edits.windows(2).any(|pair| overlap(&pair[0], &pair[1])) {
        return None;
    }
    let mut result = String::with_capacity(local.len().max(remote.len()));
    let mut cursor = 0;
    for edit in edits {
        result.push_str(&base[cursor..edit.range.start]);
        result.push_str(&edit.text);
        cursor = edit.range.end;
    }
    result.push_str(&base[cursor..]);
    Some(result)
}

fn overlap(a: &Edit, b: &Edit) -> bool {
    match (a.range.is_empty(), b.range.is_empty()) {
        (true, true) => a.range.start == b.range.start,
        (true, false) => b.range.start < a.range.start && a.range.start < b.range.end,
        (false, true) => a.range.start < b.range.start && b.range.start < a.range.end,
        (false, false) => a.range.start < b.range.end && b.range.start < a.range.end,
    }
}

fn changes(base: &str, next: &str, budget: &mut Budget) -> Option<Vec<Edit>> {
    let old: Vec<_> = base.split_inclusive('\n').take(MAX_LINES + 1).collect();
    let new: Vec<_> = next.split_inclusive('\n').take(MAX_LINES + 1).collect();
    if old.len() > MAX_LINES || new.len() > MAX_LINES {
        return None;
    }
    let mut edits = Vec::new();
    lines(&old, &new, 0, 0, budget, &mut edits)?;
    Some(edits)
}

fn lines(
    mut old: &[&str],
    mut new: &[&str],
    mut offset: usize,
    depth: usize,
    budget: &mut Budget,
    edits: &mut Vec<Edit>,
) -> Option<()> {
    budget.lines = budget.lines.checked_add(old.len() + new.len())?;
    if budget.lines > MAX_LINE_WORK {
        return None;
    }
    while !old.is_empty() && old.first() == new.first() {
        offset += old[0].len();
        old = &old[1..];
        new = &new[1..];
    }
    while !old.is_empty() && !new.is_empty() && old.last() == new.last() {
        old = &old[..old.len() - 1];
        new = &new[..new.len() - 1];
    }
    if old.is_empty() && new.is_empty() {
        return Some(());
    }
    let anchors = if depth < 32 {
        anchors(old, new)
    } else {
        Vec::new()
    };
    if anchors.is_empty() {
        scalars(&old.concat(), &new.concat(), offset, budget, edits);
        return Some(());
    }
    let (mut a, mut b) = (0, 0);
    for (i, j) in anchors {
        lines(&old[a..i], &new[b..j], offset, depth + 1, budget, edits)?;
        offset += old[a..=i].iter().map(|line| line.len()).sum::<usize>();
        a = i + 1;
        b = j + 1;
    }
    lines(&old[a..], &new[b..], offset, depth + 1, budget, edits)
}

fn anchors(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    fn unique<'a>(lines: &[&'a str]) -> HashMap<&'a str, Option<usize>> {
        let mut result = HashMap::new();
        for (i, line) in lines.iter().enumerate() {
            result
                .entry(*line)
                .and_modify(|value| *value = None)
                .or_insert(Some(i));
        }
        result
    }
    let left = unique(old);
    let right = unique(new);
    let pairs: Vec<_> = old
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            (left.get(line) == Some(&Some(i))).then_some(())?;
            right.get(line).copied().flatten().map(|j| (i, j))
        })
        .collect();
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![None; pairs.len()];
    for (i, &(_, j)) in pairs.iter().enumerate() {
        let slot = tails.partition_point(|&tail| pairs[tail].1 < j);
        previous[i] = slot.checked_sub(1).map(|slot| tails[slot]);
        if slot == tails.len() {
            tails.push(i);
        } else {
            tails[slot] = i;
        }
    }
    let mut result = Vec::with_capacity(tails.len());
    let mut cursor = tails.last().copied();
    while let Some(i) = cursor {
        result.push(pairs[i]);
        cursor = previous[i];
    }
    result.reverse();
    result
}

fn scalars(base: &str, next: &str, mut offset: usize, budget: &mut Budget, edits: &mut Vec<Edit>) {
    let prefix: usize = base
        .chars()
        .zip(next.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    offset += prefix;
    let (base, next) = (&base[prefix..], &next[prefix..]);
    let suffix: usize = base
        .chars()
        .rev()
        .zip(next.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let (base, next) = (&base[..base.len() - suffix], &next[..next.len() - suffix]);
    if base.is_empty() && next.is_empty() {
        return;
    }
    let a: Vec<_> = base.char_indices().take(MAX_SCALARS + 1).collect();
    let b: Vec<_> = next.chars().take(MAX_SCALARS + 1).collect();
    let cells = (a.len() + 1) * (b.len() + 1);
    if a.is_empty()
        || b.is_empty()
        || a.len() > MAX_SCALARS
        || b.len() > MAX_SCALARS
        || cells > MAX_MATRIX
        || budget.scalars.saturating_add(cells) > MAX_SCALAR_WORK
    {
        edits.push(Edit {
            range: offset..offset + base.len(),
            text: next.into(),
        });
        return;
    }
    budget.scalars += cells;
    let width = b.len() + 1;
    let mut lcs = vec![0u16; cells];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i * width + j] = if a[i].1 == b[j] {
                1 + lcs[(i + 1) * width + j + 1]
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let byte = |i: usize| a.get(i).map_or(base.len(), |&(byte, _)| byte);
    let (mut i, mut j) = (0, 0);
    let mut pending: Option<(usize, String)> = None;
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i].1 == b[j] {
            if let Some((start, text)) = pending.take() {
                edits.push(Edit {
                    range: offset + byte(start)..offset + byte(i),
                    text,
                });
            }
            i += 1;
            j += 1;
        } else if j < b.len()
            && (i == a.len() || lcs[i * width + j + 1] >= lcs[(i + 1) * width + j])
        {
            pending
                .get_or_insert_with(|| (i, String::new()))
                .1
                .push(b[j]);
            j += 1;
        } else {
            pending.get_or_insert_with(|| (i, String::new()));
            i += 1;
        }
    }
    if let Some((start, text)) = pending {
        edits.push(Edit {
            range: offset + byte(start)..offset + base.len(),
            text,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separate_lines_merge_without_changing_line_endings() {
        assert_eq!(
            reconcile(
                "first\r\nsecond\r\nthird",
                "FIRST\r\nsecond\r\nthird",
                "first\r\nsecond\r\nTHIRD"
            ),
            Some("FIRST\r\nsecond\r\nTHIRD".into())
        );
    }
    #[test]
    fn separate_unicode_sentence_edits_merge() {
        assert_eq!(
            reconcile(
                "α is small; β is large.",
                "α is tiny; β is large.",
                "α is small; β is huge."
            ),
            Some("α is tiny; β is huge.".into())
        );
    }
    #[test]
    fn multiple_local_changes_keep_remote_change_between_them() {
        assert_eq!(
            reconcile("a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n", "a\nb\nC\nd\ne\n"),
            Some("A\nb\nC\nd\nE\n".into())
        );
    }
    #[test]
    fn competing_insertions_and_overlapping_replacements_require_review() {
        assert_eq!(reconcile("abc", "aXbc", "aYbc"), None);
        assert_eq!(reconcile("abc", "aXc", "aYc"), None);
        assert_eq!(reconcile("abc", "ac", "aYc"), None);
        assert_eq!(reconcile("abc", "aXbc", "aXbc"), Some("aXbc".into()));
    }
    #[test]
    fn insertions_at_replacement_boundaries_are_preserved() {
        assert_eq!(reconcile("abc", "aXbc", "aBc"), Some("aXBc".into()));
        assert_eq!(reconcile("abc", "abXc", "aBc"), Some("aBXc".into()));
    }
    #[test]
    fn large_unanchored_changes_remain_conservative() {
        let base = "a".repeat(MAX_SCALARS * 2);
        assert_eq!(
            reconcile(&base, &"b".repeat(base.len()), &"c".repeat(base.len())),
            None
        );
    }
    #[test]
    fn work_limits_preserve_a_review_instead_of_guessing_a_merge() {
        let base = "x\n".repeat(MAX_LINES + 1);
        assert_eq!(
            reconcile(&base, &format!("local\n{base}"), &format!("{base}remote\n")),
            None
        );
        let mut budget = Budget {
            lines: MAX_LINE_WORK,
            scalars: 0,
        };
        assert!(changes("first\nsecond", "changed\nsecond", &mut budget).is_none());
        let mut budget = Budget {
            lines: 0,
            scalars: MAX_SCALAR_WORK,
        };
        let edits = changes("abc", "aXc", &mut budget).unwrap();
        assert_eq!(budget.scalars, MAX_SCALAR_WORK);
        assert_eq!(
            edits,
            vec![Edit {
                range: 1..2,
                text: "X".into()
            }]
        );
    }
    #[test]
    fn trivial_cases_are_exact_for_arbitrary_unicode() {
        for base in ["", "ab", "α\r\nβ", "図😀"] {
            for changed in ["", "new", "😀\n図"] {
                assert_eq!(reconcile(base, base, changed), Some(changed.into()));
                assert_eq!(reconcile(base, changed, base), Some(changed.into()));
                assert_eq!(reconcile(base, changed, changed), Some(changed.into()));
            }
        }
    }
}
