//! UI-independent unified-diff models and codecs.
use super::{Snapshot, args, diff_args, private_artifact, run, run_command};
use std::{ffi::OsString, io::Write, ops::Range, path::Path};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineChange {
    /// Zero-based buffer lines. An empty range marks a deletion at a boundary.
    pub(crate) lines: Range<usize>,
    pub(crate) kind: ChangeKind,
    /// Exact line counts on each side of the diff. Keeping both sides means an
    /// unequal replacement can report its paired modified lines and its
    /// remaining additions or deletions without losing information.
    pub(crate) old_line_count: usize,
    pub(crate) new_line_count: usize,
}

/// Line totals for the current document's Git diff. Replacement runs pair old
/// and new lines as modifications, then retain any excess on either side as
/// additions or deletions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LineChangeCounts {
    pub(crate) added: usize,
    pub(crate) modified: usize,
    pub(crate) deleted: usize,
}

impl LineChangeCounts {
    pub(crate) fn from_hunks(hunks: &[Hunk]) -> Self {
        let mut counts = Self::default();
        for change in hunks.iter().flat_map(|hunk| &hunk.changes) {
            match change.kind {
                ChangeKind::Added => counts.added += change.new_line_count,
                ChangeKind::Deleted => counts.deleted += change.old_line_count,
                ChangeKind::Modified => {
                    let paired = change.old_line_count.min(change.new_line_count);
                    counts.modified += paired;
                    counts.added += change.new_line_count - paired;
                    counts.deleted += change.old_line_count - paired;
                }
            }
        }
        counts
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hunk {
    pub(crate) text: String,
    pub(crate) changes: Vec<LineChange>,
}

fn parse_new_range(header: &str) -> Option<(usize, usize)> {
    let range = header
        .strip_prefix("@@ -")?
        .split_whitespace()
        .nth(1)?
        .strip_prefix('+')?;
    let (start, count) = range.split_once(',').unwrap_or((range, "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}

/// Parse Git's unified hunks, grouping each adjacent deletion/addition run into
/// a marker. Context lines belong to the viewer, never to the gutter highlight.
pub(in crate::git) fn parse_hunks(diff: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut cursor = 0;
    let mut added = 0;
    let mut deleted = 0;
    let flush = |hunks: &mut Vec<Hunk>, cursor: usize, added: &mut usize, deleted: &mut usize| {
        if (*added > 0 || *deleted > 0)
            && let Some(hunk) = hunks.last_mut()
        {
            let kind = if *added == 0 {
                ChangeKind::Deleted
            } else if *deleted == 0 {
                ChangeKind::Added
            } else {
                ChangeKind::Modified
            };
            hunk.changes.push(LineChange {
                lines: cursor.saturating_sub(*added)..cursor,
                kind,
                old_line_count: *deleted,
                new_line_count: *added,
            });
        }
        *added = 0;
        *deleted = 0;
    };
    for line in diff.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            flush(&mut hunks, cursor, &mut added, &mut deleted);
            let (start, count) = parse_new_range(line).ok_or("Invalid Git diff hunk header")?;
            cursor = if count == 0 {
                start
            } else {
                start.saturating_sub(1)
            };
            hunks.push(Hunk {
                text: line.to_owned(),
                changes: Vec::new(),
            });
        } else if let Some(hunk) = hunks.last_mut() {
            hunk.text.push_str(line);
            match line.as_bytes().first() {
                Some(b'+') => {
                    added += 1;
                    cursor += 1;
                }
                Some(b'-') => {
                    deleted += 1;
                }
                Some(b' ') => {
                    flush(&mut hunks, cursor, &mut added, &mut deleted);
                    cursor += 1;
                }
                // "\ No newline at end of file" does not consume a line.
                _ => {}
            }
        }
    }
    flush(&mut hunks, cursor, &mut added, &mut deleted);
    Ok(hunks)
}

pub(in crate::git) fn compare_buffer(
    root: &Path,
    before: &[u8],
    buffer: &str,
) -> Result<Vec<Hunk>, String> {
    compare_buffer_context(root, before, buffer, 3)
}

pub(in crate::git) fn compare_buffer_context(
    root: &Path,
    before: &[u8],
    buffer: &str,
    context: usize,
) -> Result<Vec<Hunk>, String> {
    if before == buffer.as_bytes() || before.contains(&0) {
        return Ok(Vec::new());
    }
    let mut old = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let mut new = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    old.write_all(before).map_err(|e| e.to_string())?;
    new.write_all(buffer.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut command = diff_args();
    command.extend(args(&["--no-index", "--no-renames"]));
    command.push(format!("--unified={context}").into());
    command.push("--".into());
    command.push(old.path().as_os_str().to_owned());
    command.push(new.path().as_os_str().to_owned());
    let diff = run_command(root, &command, true)?;
    parse_hunks(&String::from_utf8_lossy(&diff))
}

pub(in crate::git) fn buffer_hunks(
    snapshot: &Snapshot,
    path: Option<&Path>,
    buffer: &str,
) -> Result<Vec<Hunk>, String> {
    let Some(path) = path.filter(|_| snapshot.initialized) else {
        return Ok(Vec::new());
    };
    let Ok(relative) = path.strip_prefix(&snapshot.root) else {
        return Ok(Vec::new());
    };
    if private_artifact(relative) {
        return Ok(Vec::new());
    }
    let entry = snapshot.entries.iter().find(|entry| entry.path == relative);
    let new_file =
        entry.is_some_and(|entry| matches!(entry.index, '?' | 'A') || entry.worktree == 'A');
    let mut object = OsString::from("HEAD:");
    object.push(relative.as_os_str());
    let mut command = args(&["cat-file", "blob"]);
    command.push(object);
    let before = match run(&snapshot.root, &command) {
        Ok(bytes) => bytes,
        // Added files and unborn repositories have no HEAD blob. Their full
        // current buffer is the change, so a separate HEAD probe is needless.
        Err(_) if new_file => Vec::new(),
        // Ignored and unrelated files have no baseline.
        Err(_) if entry.is_none() => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    compare_buffer(&snapshot.root, &before, buffer)
}
