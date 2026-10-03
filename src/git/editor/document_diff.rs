//! Complete document alignment. Real source characters stay real editor indices;
//! unmatched lines create geometric gaps, never synthetic editable text.
use crate::git::repository::{
    diff::{Hunk, parse_hunks},
    hunks,
};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pair {
    pub(crate) old: Option<usize>,
    pub(crate) new: Option<usize>,
    pub(crate) changed: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct DocumentDiff {
    pub(crate) before: String,
    pub(crate) current: String,
    pub(crate) pairs: Vec<Pair>,
    pub(crate) hunks: Vec<Hunk>,
    pub(crate) builds: usize,
}
impl DocumentDiff {
    pub(crate) fn baseline(source: &str, changes: &[Hunk]) -> Result<String, String> {
        changes
            .iter()
            .rev()
            .try_fold(source.to_owned(), |text, hunk| hunks::revert(&text, hunk))
    }
    pub(crate) fn new(before: String, current: &str) -> Self {
        let mut result = Self {
            before,
            current: String::new(),
            pairs: vec![],
            hunks: vec![],
            builds: 0,
        };
        result.refresh(current);
        result
    }
    pub(crate) fn refresh(&mut self, current: &str) {
        if self.builds != 0 && self.current == current {
            return;
        }
        self.current = current.into();
        self.pairs = align_lines(&self.before, current);
        self.hunks = make_hunks(&self.before, current, &self.pairs);
        self.builds += 1;
    }
    pub(crate) fn destination(&self, line: usize, previous: bool) -> Option<usize> {
        let starts: Vec<_> = self
            .hunks
            .iter()
            .filter_map(|h| h.changes.first().map(|c| c.lines.start))
            .collect();
        if previous {
            starts
                .iter()
                .rev()
                .copied()
                .find(|&start| start < line)
                .or_else(|| starts.last().copied())
        } else {
            starts
                .iter()
                .copied()
                .find(|&start| start > line)
                .or_else(|| starts.first().copied())
        }
    }
    pub(crate) fn revert_line(&self, pair: usize) -> Option<String> {
        let row = self.pairs.get(pair).filter(|p| p.changed)?;
        let old: Vec<_> = self.before.split_inclusive('\n').collect();
        let new: Vec<_> = self.current.split_inclusive('\n').collect();
        let at = row.new.unwrap_or_else(|| {
            self.pairs[..pair]
                .iter()
                .filter(|p| p.new.is_some())
                .count()
        });
        let mut lines = new[..at]
            .iter()
            .copied()
            .chain(row.old.map(|line| old[line]))
            .chain(new[at + usize::from(row.new.is_some())..].iter().copied())
            .peekable();
        let mut result = String::with_capacity(self.current.len());
        while let Some(line) = lines.next() {
            result.push_str(line);
            // A baseline EOF without a newline can now sit in the middle of
            // the edited document. Preserve logical line boundaries there.
            if lines.peek().is_some() && !result.ends_with('\n') {
                result.push('\n');
            }
        }
        Some(result)
    }
}

// Myers with prefix/suffix trimming and a bounded trace. Very different large
// files fall back to a valid coarse replacement rather than quadratic memory.
fn matches(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    if a.is_empty() || b.is_empty() {
        return vec![];
    }
    let max = a.len() + b.len();
    let offset = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    let mut trace = Vec::new();
    for d in 0..=max.min(512) {
        if (trace.len() + 1) * v.len() > 4_000_000 {
            break;
        }
        trace.push(v.clone());
        for k in (-(d as isize)..=d as isize).step_by(2) {
            let i = (offset + k) as usize;
            let mut x = if k == -(d as isize) || (k != d as isize && v[i - 1] < v[i + 1]) {
                v[i + 1]
            } else {
                v[i - 1] + 1
            };
            let mut y = x - k;
            while x < a.len() as isize && y < b.len() as isize && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x == a.len() as isize && y == b.len() as isize {
                let mut equal = Vec::new();
                for depth in (0..=d).rev() {
                    let prior = &trace[depth];
                    let k = x - y;
                    let i = (offset + k) as usize;
                    let prev_k = if k == -(depth as isize)
                        || (k != depth as isize && prior[i - 1] < prior[i + 1])
                    {
                        k + 1
                    } else {
                        k - 1
                    };
                    let prev_x = prior[(offset + prev_k) as usize];
                    let prev_y = prev_x - prev_k;
                    while x > prev_x && y > prev_y {
                        x -= 1;
                        y -= 1;
                        equal.push((x as usize, y as usize));
                    }
                    x = prev_x;
                    y = prev_y;
                }
                equal.reverse();
                return equal;
            }
        }
    }
    vec![]
}
fn align_lines(before: &str, current: &str) -> Vec<Pair> {
    let a: Vec<_> = before.split_inclusive('\n').collect();
    let b: Vec<_> = current.split_inclusive('\n').collect();
    let prefix = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let equal = (0..prefix)
        .map(|i| (i, i))
        .chain(
            matches(&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix])
                .into_iter()
                .map(|(i, j)| (i + prefix, j + prefix)),
        )
        .chain((0..suffix).map(|i| (a.len() - suffix + i, b.len() - suffix + i)))
        .chain(std::iter::once((a.len(), b.len())));
    let mut rows = Vec::new();
    let (mut old, mut new) = (0, 0);
    for (i, j) in equal {
        for n in 0..(i - old).max(j - new) {
            rows.push(Pair {
                old: (old + n < i).then_some(old + n),
                new: (new + n < j).then_some(new + n),
                changed: true,
            });
        }
        if i < a.len() {
            rows.push(Pair {
                old: Some(i),
                new: Some(j),
                changed: false,
            });
        }
        old = i + 1;
        new = j + 1;
    }
    rows
}
fn make_hunks(before: &str, current: &str, pairs: &[Pair]) -> Vec<Hunk> {
    let old: Vec<_> = before.split_inclusive('\n').collect();
    let new: Vec<_> = current.split_inclusive('\n').collect();
    let mut ranges: Vec<Range<usize>> = vec![];
    for (i, pair) in pairs.iter().enumerate().filter(|(_, p)| p.changed) {
        let _ = pair;
        let range = i.saturating_sub(3)..(i + 4).min(pairs.len());
        if let Some(last) = ranges.last_mut().filter(|last| last.end >= range.start) {
            last.end = range.end;
        } else {
            ranges.push(range);
        }
    }
    let mut text = String::new();
    let (mut through, mut a, mut b) = (0, 0, 0);
    for range in ranges {
        // Ranges are ordered and disjoint: advance each line offset once,
        // instead of rescanning the entire prefix for every separated hunk.
        for pair in &pairs[through..range.start] {
            a += usize::from(pair.old.is_some());
            b += usize::from(pair.new.is_some());
        }
        let ac = pairs[range.clone()]
            .iter()
            .filter(|p| p.old.is_some())
            .count();
        let bc = pairs[range.clone()]
            .iter()
            .filter(|p| p.new.is_some())
            .count();
        text.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            a + usize::from(ac != 0),
            ac,
            b + usize::from(bc != 0),
            bc
        ));
        let add = |text: &mut String, prefix: char, line: &str| {
            text.push(prefix);
            text.push_str(line);
            if !line.ends_with('\n') {
                text.push_str("\n\\ No newline at end of file\n");
            }
        };
        let mut at = range.start;
        while at < range.end {
            if !pairs[at].changed {
                add(&mut text, ' ', old[pairs[at].old.unwrap()]);
                at += 1;
                continue;
            }
            let start = at;
            while at < range.end && pairs[at].changed {
                at += 1;
            }
            for row in &pairs[start..at] {
                if let Some(i) = row.old {
                    add(&mut text, '-', old[i]);
                }
            }
            for row in &pairs[start..at] {
                if let Some(i) = row.new {
                    add(&mut text, '+', new[i]);
                }
            }
        }
        through = range.end;
        a += ac;
        b += bc;
    }
    parse_hunks(&text).expect("generated valid unified ranges")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reverting_one_line_keeps_adjacent_lines_separate_without_final_newlines() {
        let diff = DocumentDiff::new("old".into(), "changed\nnext\n");
        assert_eq!(diff.revert_line(0).unwrap(), "old\nnext\n");
        let diff = DocumentDiff::new("one\ntwo\n".into(), "changed");
        assert_eq!(diff.revert_line(1).unwrap(), "changed\ntwo\n");
        let diff = DocumentDiff::new("old".into(), "changed");
        assert_eq!(diff.revert_line(0).unwrap(), "old");
    }
    #[test]
    fn sparse_changes_keep_correct_offsets_across_many_hunks() {
        let before = (0..10_000)
            .map(|i| format!("line {i:05} original\n"))
            .collect::<String>();
        let current = (0..10_000)
            .map(|i| {
                format!(
                    "line {i:05} {}\n",
                    if i % 300 == 150 {
                        "changed"
                    } else {
                        "original"
                    }
                )
            })
            .collect::<String>();
        let diff = DocumentDiff::new(before.clone(), &current);
        assert_eq!(diff.hunks.len(), 33);
        assert_eq!(
            DocumentDiff::baseline(&current, &diff.hunks).unwrap(),
            before
        );
    }
    #[test]
    fn very_different_large_documents_use_a_bounded_coarse_alignment() {
        let before = (0..10_000)
            .map(|i| format!("old {i}\n"))
            .collect::<String>();
        let current = (0..10_000)
            .map(|i| format!("new {i}\n"))
            .collect::<String>();
        let diff = DocumentDiff::new(before.clone(), &current);
        assert_eq!(diff.pairs.len(), 10_000);
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(
            DocumentDiff::baseline(&current, &diff.hunks).unwrap(),
            before
        );
    }

    #[test]
    #[ignore = "Opt-in same-profile benchmark of old Git process diff and live in-memory diff"]
    fn document_diff_measurement() {
        use std::time::Instant;
        let directory = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(directory.path())
            .status()
            .unwrap();
        let before = (0..4_000)
            .map(|i| format!("Line {i:04}: mathematical prose and alpha beta gamma\n"))
            .collect::<String>();
        let variants: Vec<_> = (0..24)
            .map(|i| before.replacen("Line 2000:", &format!("Line 2000: edit {i}"), 1))
            .collect();
        // Warm both implementations with the same document and edits.
        crate::git::repository::diff::compare_buffer(
            directory.path(),
            before.as_bytes(),
            &variants[0],
        )
        .unwrap();
        DocumentDiff::new(before.clone(), &variants[0]);
        let start = Instant::now();
        for source in &variants {
            std::hint::black_box(
                crate::git::repository::diff::compare_buffer(
                    directory.path(),
                    before.as_bytes(),
                    source,
                )
                .unwrap(),
            );
        }
        let old = start.elapsed();
        let mut model = DocumentDiff::new(before.clone(), &variants[0]);
        let start = Instant::now();
        for source in &variants {
            model.refresh(source);
            std::hint::black_box(&model);
        }
        let new = start.elapsed();
        let start = Instant::now();
        for _ in 0..1000 {
            model.refresh(variants.last().unwrap());
        }
        let idle = start.elapsed();
        println!(
            "document_diff_measurement profile=release fixture=4000-lines edits=24 warmup=1 old_git_ms={} live_diff_ms={} cache_hits_1000_ms={} model_builds={}",
            old.as_secs_f64() * 1000.0,
            new.as_secs_f64() * 1000.0,
            idle.as_secs_f64() * 1000.0,
            model.builds
        );
    }

    #[test]
    fn all_lines_survive_and_edits_realign_immediately() {
        let before: String = (0..180).map(|i| format!("line {i}\n")).collect();
        let current = before
            .replace("line 5\n", "changed\nextra\n")
            .replace("line 165\n", "");
        let mut diff = DocumentDiff::new(before.clone(), &current);
        assert_eq!(
            diff.pairs.iter().filter_map(|r| r.old).collect::<Vec<_>>(),
            (0..180).collect::<Vec<_>>()
        );
        assert_eq!(
            diff.pairs.iter().filter_map(|r| r.new).collect::<Vec<_>>(),
            (0..180).collect::<Vec<_>>()
        );
        assert_eq!(
            DocumentDiff::baseline(&current, &diff.hunks).unwrap(),
            before
        );
        assert_eq!(diff.destination(5, false), Some(166));
        diff.refresh(&current);
        assert_eq!(diff.builds, 1);
        diff.refresh(&before);
        assert!(diff.hunks.is_empty());
        assert_eq!(diff.builds, 2);
    }
    #[test]
    fn insertions_deletions_unicode_and_final_newlines_revert_exactly() {
        for (a, b) in [
            ("", "α\n"),
            ("α\n", ""),
            ("α", "β\n"),
            ("a\nb\nc\n", "a\nz\nb\nc\n"),
            ("same\nrepeat\nsame\n", "same\nsame\n"),
        ] {
            let diff = DocumentDiff::new(a.into(), b);
            assert_eq!(DocumentDiff::baseline(b, &diff.hunks).unwrap(), a);
            let changed = diff.pairs.iter().position(|r| r.changed).unwrap();
            assert!(diff.revert_line(changed).is_some());
        }
    }
    #[test]
    fn exhaustive_short_sequences_preserve_every_line_and_checked_baseline() {
        let texts: Vec<String> = (0..81)
            .map(|mut n| {
                let mut s = String::new();
                for _ in 0..4 {
                    let c = n % 3;
                    n /= 3;
                    if c != 0 {
                        s.push_str(if c == 1 { "a\n" } else { "b\n" });
                    }
                }
                s
            })
            .collect();
        for a in &texts {
            for b in &texts {
                let diff = DocumentDiff::new(a.clone(), b);
                assert_eq!(
                    DocumentDiff::baseline(b, &diff.hunks).unwrap(),
                    *a,
                    "{a:?} {b:?}"
                );
                assert_eq!(
                    diff.pairs.iter().filter_map(|r| r.old).collect::<Vec<_>>(),
                    (0..a.lines().count()).collect::<Vec<_>>()
                );
                assert_eq!(
                    diff.pairs.iter().filter_map(|r| r.new).collect::<Vec<_>>(),
                    (0..b.lines().count()).collect::<Vec<_>>()
                );
            }
        }
    }
}
