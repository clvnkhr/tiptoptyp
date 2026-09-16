//! Hunk mutations are explicit, worker-only index transactions. The working
//! file is never written: reverting returns a checked replacement for Undo.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Stage,
    Unstage,
    Revert,
    Previous,
    Next,
}

impl Action {
    pub(crate) const ALL: [Self; 5] = [
        Self::Previous,
        Self::Next,
        Self::Stage,
        Self::Unstage,
        Self::Revert,
    ];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage",
            Self::Unstage => "Unstage",
            Self::Revert => "Revert",
            Self::Previous => "Previous",
            Self::Next => "Next",
        }
    }
    pub(crate) fn shortcut(self) -> crate::shortcuts::ShortcutAction {
        use crate::shortcuts::ShortcutAction as S;
        match self {
            Self::Stage => S::StageHunk,
            Self::Unstage => S::UnstageHunk,
            Self::Revert => S::RevertHunk,
            Self::Previous => S::PreviousHunk,
            Self::Next => S::NextHunk,
        }
    }
}

#[derive(Debug)]
struct Edit {
    old: Range<usize>,
    new: Range<usize>,
    before: String,
    after: String,
}

fn range(field: &str) -> Result<Range<usize>, String> {
    let (start, count) = field.split_once(',').unwrap_or((field, "1"));
    let start = start.parse::<usize>().map_err(|e| e.to_string())?;
    let count = count.parse::<usize>().map_err(|e| e.to_string())?;
    let start = if count == 0 {
        start
    } else {
        start.saturating_sub(1)
    };
    Ok(start..start + count)
}

fn edit(hunk: &Hunk) -> Result<Edit, String> {
    let mut lines = hunk.text.split_inclusive('\n');
    let header = lines.next().ok_or("Missing hunk header")?;
    let fields: Vec<_> = header.split_whitespace().collect();
    let old = range(
        fields
            .get(1)
            .and_then(|s| s.strip_prefix('-'))
            .ok_or("Invalid old range")?,
    )?;
    let new = range(
        fields
            .get(2)
            .and_then(|s| s.strip_prefix('+'))
            .ok_or("Invalid new range")?,
    )?;
    let (mut before, mut after) = (String::new(), String::new());
    let mut previous = b' ';
    for line in lines {
        match line.as_bytes().first().copied() {
            Some(b'\\') => {
                if previous != b'+' {
                    before.pop();
                }
                if previous != b'-' {
                    after.pop();
                }
            }
            Some(kind @ (b' ' | b'+' | b'-')) => {
                if kind != b'+' {
                    before.push_str(&line[1..]);
                }
                if kind != b'-' {
                    after.push_str(&line[1..]);
                }
                previous = kind;
            }
            _ => return Err("Invalid hunk contents".into()),
        }
    }
    // Context lines must not sweep unrelated index edits into this action.
    let a: Vec<_> = before.split_inclusive('\n').collect();
    let b: Vec<_> = after.split_inclusive('\n').collect();
    let prefix = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    Ok(Edit {
        old: old.start + prefix..old.end - suffix,
        new: new.start + prefix..new.end - suffix,
        before: a[prefix..a.len() - suffix].concat(),
        after: b[prefix..b.len() - suffix].concat(),
    })
}

fn bytes_for_lines(source: &str, range: Range<usize>) -> Result<Range<usize>, String> {
    let mut starts: Vec<_> = source
        .split_inclusive('\n')
        .scan(0, |offset, line| {
            let start = *offset;
            *offset += line.len();
            Some(start)
        })
        .collect();
    starts.push(source.len());
    Ok(*starts.get(range.start).ok_or("Stale hunk range")?
        ..*starts.get(range.end).ok_or("Stale hunk range")?)
}

pub(crate) fn revert(source: &str, hunk: &Hunk) -> Result<String, String> {
    let edit = edit(hunk)?;
    let bytes = bytes_for_lines(source, edit.new)?;
    if source[bytes.clone()] != edit.after {
        return Err("The hunk changed; reopen it before reverting".into());
    }
    let mut result = source.to_owned();
    result.replace_range(bytes, &edit.before);
    Ok(result)
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    if a.is_empty() && b.is_empty() {
        a.start == b.start
    } else if a.is_empty() {
        b.contains(&a.start)
    } else if b.is_empty() {
        a.contains(&b.start)
    } else {
        a.start < b.end && b.start < a.end
    }
}

fn selected_index(
    root: &Path,
    head: &str,
    index: &str,
    selected: Edit,
    stage: bool,
) -> Result<String, String> {
    let mut edits = Vec::new();
    for hunk in compare_buffer_context(root, head.as_bytes(), index, 0)? {
        let existing = edit(&hunk)?;
        if overlaps(&existing.old, &selected.old) {
            if existing.old.start < selected.old.start || existing.old.end > selected.old.end {
                return Err("A staged change crosses this hunk's boundary; unstage that overlapping change first".into());
            }
        } else {
            edits.push(existing);
        }
    }
    if stage {
        edits.push(selected);
    }
    edits.sort_by_key(|edit| edit.old.start);
    let mut result = head.to_owned();
    for edit in edits.into_iter().rev() {
        let bytes = bytes_for_lines(head, edit.old)?;
        if head[bytes.clone()] != edit.before {
            return Err("The Git baseline changed; reopen the hunk".into());
        }
        result.replace_range(bytes, &edit.after);
    }
    Ok(result)
}

// Git's C-quoted paths, including control characters and non-ASCII bytes.
fn quote_path(prefix: &str, path: &Path) -> String {
    let mut result = format!("\"{prefix}");
    for &byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'"' | b'\\' => {
                result.push('\\');
                result.push(byte as char);
            }
            32..=126 => result.push(byte as char),
            _ => result.push_str(&format!("\\{byte:03o}")),
        }
    }
    result.push('"');
    result
}

pub(crate) fn change_index(
    workspace: &Path,
    path: &Path,
    source: &str,
    hunk: &Hunk,
    action: Action,
) -> Result<String, String> {
    let snapshot = status_snapshot(workspace)?;
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    let path = canonical.as_path();
    let relative = path
        .strip_prefix(&snapshot.root)
        .map_err(|_| "File is outside the repository")?;
    if private_artifact(relative) {
        return Err("Private artifacts cannot be staged".into());
    }
    let entry = snapshot.entries.iter().find(|entry| entry.path == relative);
    if entry.is_some_and(|e| {
        e.index == 'U'
            || e.worktree == 'U'
            || matches!((e.index, e.worktree), ('A', 'A') | ('D', 'D'))
    }) {
        return Err("Resolve the merge conflict before using hunk actions".into());
    }
    let blob = |prefix: &str| {
        let mut object = OsString::from(prefix);
        object.push(relative);
        run(
            &snapshot.root,
            &[OsString::from("cat-file"), OsString::from("blob"), object],
        )
    };
    let (head, head_exists) = match blob("HEAD:") {
        Ok(bytes) => (bytes, true),
        Err(_) if entry.is_some_and(|e| matches!(e.index, '?' | 'A')) => (Vec::new(), false),
        Err(error) => return Err(error),
    };
    let index = match blob(":") {
        Ok(bytes) => Some(bytes),
        Err(_) if entry.is_some_and(|e| matches!(e.index, '?' | 'D')) => None,
        Err(error) => return Err(error),
    };
    let head = std::str::from_utf8(&head).map_err(|e| e.to_string())?;
    let index_source =
        std::str::from_utf8(index.as_deref().unwrap_or_default()).map_err(|e| e.to_string())?;
    // Validate against current HEAD, not just the buffer's line coordinates.
    if !compare_buffer(&snapshot.root, head.as_bytes(), source)?
        .iter()
        .any(|candidate| candidate.text == hunk.text)
    {
        return Err("The Git baseline changed; reopen the hunk".into());
    }
    let desired = selected_index(
        &snapshot.root,
        head,
        index_source,
        edit(hunk)?,
        action == Action::Stage,
    )?;
    if desired == index_source {
        return Ok("This hunk already has the requested staging state".into());
    }
    let hunks = compare_buffer(&snapshot.root, index_source.as_bytes(), &desired)?;
    let old_path = if index.is_none() {
        "/dev/null".to_owned()
    } else {
        quote_path("a/", relative)
    };
    let new_path = if !head_exists && desired.is_empty() && action == Action::Unstage {
        "/dev/null".into()
    } else {
        quote_path("b/", relative)
    };
    let mut patch = format!("--- {old_path}\n+++ {new_path}\n");
    #[cfg(unix)]
    if index.is_none() {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path)
            .map_err(|e| e.to_string())?
            .permissions()
            .mode()
            & 0o111
            != 0
        {
            patch = format!(
                "diff --git {} {}\nnew file mode 100755\n{patch}",
                quote_path("a/", relative),
                quote_path("b/", relative)
            );
        }
    }
    for hunk in hunks {
        patch.push_str(&hunk.text);
    }
    let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    file.write_all(patch.as_bytes())
        .map_err(|e| e.to_string())?;
    run(
        &snapshot.root,
        &[
            "apply".into(),
            "--cached".into(),
            "--whitespace=nowarn".into(),
            "--".into(),
            file.path().as_os_str().to_owned(),
        ],
    )?;
    Ok(format!(
        "{}d hunk in {}",
        if action == Action::Stage {
            "Stage"
        } else {
            "Unstage"
        },
        relative.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn partial_staging_preserves_other_hunks_and_working_file() {
        let root = tempfile::tempdir().unwrap();
        let original = (0..30).map(|i| format!("line {i}\n")).collect::<String>();
        super::super::tests::initialize(root.path(), &original);
        let path = root.path().join("main.typ");
        let staged = original
            .replace("line 2\n", "staged\n")
            .replace("line 25\n", "other staged\n");
        fs::write(&path, &staged).unwrap();
        run(root.path(), &args(&["add", "main.typ"])).unwrap();
        let buffer = staged.replace("staged\nline 3", "buffer\nline 3");
        let hunks = compare_buffer(root.path(), original.as_bytes(), &buffer).unwrap();
        assert_eq!(hunks.len(), 2);
        change_index(root.path(), &path, &buffer, &hunks[0], Action::Stage).unwrap();
        assert_eq!(
            run(root.path(), &args(&["show", ":main.typ"])).unwrap(),
            buffer.as_bytes()
        );
        change_index(root.path(), &path, &buffer, &hunks[0], Action::Unstage).unwrap();
        assert_eq!(
            run(root.path(), &args(&["show", ":main.typ"])).unwrap(),
            original.replace("line 25\n", "other staged\n").as_bytes()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), staged);
        assert_eq!(
            revert(&buffer, &hunks[0]).unwrap(),
            original.replace("line 25\n", "other staged\n")
        );
    }

    #[test]
    fn revert_round_trips_unicode_empty_files_crlf_and_missing_newline() {
        let root = tempfile::tempdir().unwrap();
        for (before, after) in [
            ("", "α"),
            ("α", ""),
            ("a\r\nb\r\n", "a\r\nβ\r\n"),
            ("a\nb", "a\nb\n"),
            ("a\nb\n", "a\nb"),
        ] {
            let hunks = compare_buffer(root.path(), before.as_bytes(), after).unwrap();
            assert_eq!(hunks.len(), 1);
            assert_eq!(revert(after, &hunks[0]).unwrap(), before);
            if !after.is_empty() {
                assert!(revert("stale", &hunks[0]).is_err());
            }
        }
    }

    #[test]
    fn unborn_quoted_paths_stage_and_unstage_without_touching_disk() {
        let root = tempfile::tempdir().unwrap();
        run(root.path(), &args(&["init"])).unwrap();
        let path = root.path().join("α \"draft\"\t.typ");
        fs::write(&path, "saved").unwrap();
        let buffer = "unsaved\nα";
        let hunk = compare_buffer(root.path(), b"", buffer).unwrap().remove(0);
        change_index(root.path(), &path, buffer, &hunk, Action::Stage).unwrap();
        let object = format!(":{}", path.file_name().unwrap().to_str().unwrap());
        assert_eq!(
            run(root.path(), &args(&["show", &object])).unwrap(),
            buffer.as_bytes()
        );
        change_index(root.path(), &path, buffer, &hunk, Action::Unstage).unwrap();
        assert!(
            run(root.path(), &args(&["ls-files", "--stage"]))
                .unwrap()
                .is_empty()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "saved");
    }

    #[cfg(unix)]
    #[test]
    fn staging_a_new_executable_preserves_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        run(root.path(), &args(&["init"])).unwrap();
        let path = root.path().join("script.sh");
        let source = "#!/bin/sh\nexit 0\n";
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let hunk = compare_buffer(root.path(), b"", source).unwrap().remove(0);
        change_index(root.path(), &path, source, &hunk, Action::Stage).unwrap();
        assert!(
            run(root.path(), &args(&["ls-files", "--stage"]))
                .unwrap()
                .starts_with(b"100755 ")
        );
        assert_eq!(fs::read_to_string(path).unwrap(), source);
    }

    #[test]
    fn changed_head_and_boundary_crossing_staged_edits_are_rejected_atomically() {
        let root = tempfile::tempdir().unwrap();
        let before = "one\ntwo\nthree\nfour\n";
        super::super::tests::initialize(root.path(), before);
        let path = root.path().join("main.typ");
        let buffer = before.replace("two", "changed");
        let hunk = compare_buffer(root.path(), before.as_bytes(), &buffer)
            .unwrap()
            .remove(0);
        let staged = "one\ncombined\nfour\n";
        fs::write(&path, staged).unwrap();
        run(root.path(), &args(&["add", "main.typ"])).unwrap();
        for action in [Action::Stage, Action::Unstage] {
            assert!(
                change_index(root.path(), &path, &buffer, &hunk, action)
                    .unwrap_err()
                    .contains("boundary")
            );
            assert_eq!(
                run(root.path(), &args(&["show", ":main.typ"])).unwrap(),
                staged.as_bytes()
            );
        }
        run(root.path(), &args(&["commit", "-m", "changed baseline"])).unwrap();
        assert!(
            change_index(root.path(), &path, &buffer, &hunk, Action::Stage)
                .unwrap_err()
                .contains("baseline")
        );
    }

    #[test]
    fn projected_revert_is_an_undoable_display_edit_with_canonical_round_trip() {
        use tiptoptyp::{mitex_document::Document, mitex_projection::Config};
        use tiptoptyp_core::{
            document::{DocumentKind, WindowSessionId},
            text::{AppliedTextEdits, ScalarOffset},
        };
        let root = tempfile::tempdir().unwrap();
        let before = "#import \"@preview/mitex:0.2.7\": mi, mitex\n#mitex(`\\alpha`)\n";
        let mut document = Document::new(
            WindowSessionId::new(4),
            before.to_owned(),
            DocumentKind::Typst,
        );
        document.enable(Config::default()).unwrap();
        document.edit(0usize, |source| *source = source.replace("alpha", "beta"));
        let after = document.canonical_snapshot().unwrap().source().to_owned();
        let hunk = compare_buffer(root.path(), before.as_bytes(), &after)
            .unwrap()
            .remove(0);
        let text = revert(document.canonical_snapshot().unwrap().source(), &hunk).unwrap();
        let edit = document
            .project_canonical_change(
                document.key(),
                AppliedTextEdits {
                    text,
                    mapped_offsets: [ScalarOffset::new(0); 2],
                },
            )
            .unwrap();
        document.edit(0usize, |source| *source = edit.text);
        assert_eq!(document.canonical_snapshot().unwrap().source(), before);
        document.history_step(false, 0).unwrap();
        assert_eq!(document.canonical_snapshot().unwrap().source(), after);
    }
}
