//! Per-document-window Git decorations. Workers carry the workspace, file and
//! buffer revision; the selected chunk owns its contents independently.
use super::repository::diff::{ChangeKind, Hunk, LineChange, LineChangeCounts};
#[cfg(test)]
use super::repository::diff::{buffer_hunks, parse_hunks};
#[cfg(test)]
use super::repository::text_run;
use super::repository::{BufferStatus, Entry, Repository, Snapshot, private_artifact};
use crate::{
    document::DocumentKey,
    theme,
    worker::{LatestJob, LatestJobPoll},
};
use eframe::egui;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

mod actions;
pub(crate) mod view;

const EDIT_DEBOUNCE: Duration = Duration::from_millis(180);
/// Reserved space for the Git change marker beside the line-number gutter.
///
/// Keep this close to one editor character so the decoration does not create
/// a visibly oversized blank strip when line numbers are enabled.
pub(crate) const GUTTER_WIDTH: i8 = 6;
const GUTTER_HIT_WIDTH: f32 = GUTTER_WIDTH as f32;

impl ChangeKind {
    pub(crate) fn color(self, context: &egui::Context) -> egui::Color32 {
        let palette = theme::palette(context);
        match self {
            Self::Added => palette.success,
            Self::Modified => palette.info,
            Self::Deleted => palette.error,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStatus {
    index: char,
    worktree: char,
}

impl FileStatus {
    pub(crate) fn letter(self) -> &'static str {
        if self.index == 'U'
            || self.worktree == 'U'
            || matches!((self.index, self.worktree), ('A', 'A') | ('D', 'D'))
        {
            "U"
        } else if self.index == '?' {
            "?"
        } else if self.index == 'D' || self.worktree == 'D' {
            "D"
        } else if self.index == 'A' || self.worktree == 'A' {
            "A"
        } else if self.index == 'T' || self.worktree == 'T' {
            "T"
        } else {
            "M"
        }
    }

    pub(crate) fn description(self) -> String {
        let label = match self.letter() {
            "U" => "Merge conflict",
            "?" => "Untracked",
            "D" => "Deleted",
            "A" => "Added",
            "T" => "File type changed",
            _ => "Modified",
        };
        let detail = if self.index == '?' {
            "not staged"
        } else if self.index != ' ' && self.worktree != ' ' {
            "staged and unstaged changes"
        } else if self.index != ' ' {
            "staged"
        } else {
            "unstaged changes"
        };
        format!("{label} · {detail}")
    }

    pub(crate) fn color(self, context: &egui::Context) -> egui::Color32 {
        match self.letter() {
            "U" => theme::palette(context).warning,
            "A" | "?" => ChangeKind::Added.color(context),
            "D" => ChangeKind::Deleted.color(context),
            _ => ChangeKind::Modified.color(context),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FileStatuses(BTreeMap<PathBuf, FileStatus>);

impl FileStatuses {
    fn from_snapshot(snapshot: &Snapshot) -> Self {
        Self(
            snapshot
                .entries
                .iter()
                .map(|entry| {
                    (
                        snapshot.root.join(&entry.path),
                        FileStatus {
                            index: entry.index,
                            worktree: entry.worktree,
                        },
                    )
                })
                .collect(),
        )
    }

    pub(crate) fn get(&self, path: &Path) -> Option<FileStatus> {
        self.0.get(path).copied()
    }
}

impl LineChange {
    pub(crate) fn label(&self) -> String {
        if self.lines.is_empty() {
            format!("Git change: deleted lines at line {}", self.lines.start + 1)
        } else {
            format!(
                "Git change: {} lines {}–{}",
                self.kind.label(),
                self.lines.start + 1,
                self.lines.end
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestKey {
    workspace: PathBuf,
    path: Option<PathBuf>,
    document: DocumentKey,
}

struct ScanResult {
    key: RequestKey,
    repository: Option<PathBuf>,
    status: FileStatuses,
    hunks: Result<Vec<Hunk>, String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkDiff {
    pub(crate) path: PathBuf,
    pub(crate) hunk: Hunk,
}

pub(crate) struct GitEditorState {
    current: Option<RequestKey>,
    repository: Option<PathBuf>,
    job: LatestJob<ScanResult>,
    pub(crate) statuses: FileStatuses,
    pub(crate) hunks: Vec<Hunk>,
    pub(crate) chunk: Option<ChunkDiff>,
    scan_deadline: Instant,
    refresh_requested: bool,
}

impl Default for GitEditorState {
    fn default() -> Self {
        Self {
            current: None,
            repository: None,
            job: LatestJob::default(),
            statuses: FileStatuses::default(),
            hunks: Vec::new(),
            chunk: None,
            scan_deadline: Instant::now(),
            refresh_requested: true,
        }
    }
}

impl GitEditorState {
    pub(crate) fn activity(&self) -> crate::activity::Activity {
        if self.refresh_requested
            && !matches!(self.job.activity(), crate::activity::Activity::Failed(_))
        {
            crate::activity::Activity::Pending("Hunks stale; waiting for edits to settle")
        } else {
            self.job.activity()
        }
    }
    pub(crate) fn clear_document(&mut self) {
        self.hunks.clear();
        self.chunk = None;
        self.request_refresh();
    }
    /// Request a scan after a filesystem or Git operation changes the
    /// repository. Git decorations are event driven; the editor does not
    /// wake up on a periodic timer just to rediscover an unchanged snapshot.
    pub(crate) fn request_refresh(&mut self) {
        self.refresh_requested = true;
        self.scan_deadline = Instant::now();
    }

    pub(crate) fn line_change_counts(&self) -> LineChangeCounts {
        LineChangeCounts::from_hunks(&self.hunks)
    }

    pub(crate) fn has_gutter(&self, path: Option<&Path>) -> bool {
        self.repository.as_ref().is_some_and(|root| {
            path.is_some_and(|path| path.starts_with(root) && !private_artifact(path))
        })
    }

    pub(crate) fn tick(
        &mut self,
        context: &egui::Context,
        workspace: &Path,
        path: Option<&Path>,
        document: DocumentKey,
        source: &str,
        projection: Option<&tiptoptyp::mitex_document::CanonicalSnapshot>,
    ) {
        let key = RequestKey {
            workspace: workspace.into(),
            path: path.map(Path::to_path_buf),
            document,
        };
        let now = Instant::now();
        self.prepare_request(&key, now);
        match self.job.poll() {
            LatestJobPoll::Ready(result) => self.accept(result),
            LatestJobPoll::Failed(_) => {
                // A missing Git executable or a temporarily unavailable repo
                // should not interrupt editing or leave obsolete decorations.
                self.statuses = FileStatuses::default();
                self.repository = None;
                self.hunks.clear();
            }
            LatestJobPoll::Idle | LatestJobPoll::Pending => {}
        }
        if !self.job.is_running() && self.refresh_requested && now >= self.scan_deadline {
            let source = projection
                .map_or(source, |snapshot| snapshot.source())
                .to_owned();
            let projection = projection.cloned();
            self.refresh_requested = false;
            let work = move || {
                let BufferStatus { snapshot, hunks } =
                    Repository::new(&key.workspace).scan_buffer(key.path.as_deref(), &source)?;
                let hunks = hunks.map(|mut hunks| {
                    if let Some(projection) = projection {
                        for hunk in &mut hunks {
                            for change in &mut hunk.changes {
                                change.lines = projection.editor_lines(change.lines.clone());
                            }
                        }
                    }
                    hunks
                });
                Ok(ScanResult {
                    repository: snapshot.initialized.then(|| snapshot.root.clone()),
                    status: FileStatuses::from_snapshot(&snapshot),
                    hunks,
                    key,
                })
            };
            let result = self.job.start_and_repaint("git-editor", context, work);
            if result.is_err() {
                self.refresh_requested = true;
            }
        }
        if self.refresh_requested && !self.job.is_running() {
            context.request_repaint_after(self.scan_deadline.saturating_duration_since(now));
        }
    }

    fn prepare_request(&mut self, key: &RequestKey, now: Instant) {
        if self.current.as_ref() != Some(key) {
            self.refresh_requested = true;
            let same_file = self
                .current
                .as_ref()
                .is_some_and(|old| old.workspace == key.workspace && old.path == key.path);
            let same_document = self.current.as_ref().is_some_and(|old| {
                same_file
                    && old.document.owner == key.document.owner
                    && old.document.epoch == key.document.epoch
            });
            if !same_file {
                // An obsolete repository scan must not delay decorations in
                // the newly opened file. Ordinary edits retain the existing
                // worker so typing cannot spawn a stream of competing scans.
                self.job.supersede();
            }
            if self
                .current
                .as_ref()
                .is_none_or(|old| old.workspace != key.workspace)
            {
                self.statuses = FileStatuses::default();
                self.repository = None;
            }
            if !same_document {
                // Keep stale markers visible while an edit's debounced scan
                // is pending. A completed scan replaces them atomically;
                // document replacements must not inherit another buffer's
                // decorations.
                self.hunks.clear();
            }
            // A selected chunk is tied to the exact buffer revision that
            // produced it. Keep stale text from surviving a file switch or a
            // subsequent edit while the replacement scan is pending.
            self.chunk = None;
            self.scan_deadline = now
                + if same_file {
                    EDIT_DEBOUNCE
                } else {
                    Duration::ZERO
                };
            self.current = Some(key.clone());
        }
    }

    fn accept(&mut self, result: ScanResult) {
        if self
            .current
            .as_ref()
            .is_some_and(|key| key.workspace == result.key.workspace)
        {
            self.statuses = result.status;
            self.repository = result.repository;
            if self.current.as_ref() == Some(&result.key) {
                self.hunks = result.hunks.unwrap_or_default();
            }
        }
    }

    pub(crate) fn open_chunk(&mut self, index: usize, path: &Path) {
        if let Some(hunk) = self.hunks.get(index) {
            self.chunk = Some(ChunkDiff {
                path: path.into(),
                hunk: hunk.clone(),
            });
        }
    }

    pub(crate) fn selection_is_current(&self, key: DocumentKey, chunk: &ChunkDiff) -> bool {
        self.current.as_ref().is_some_and(|current| {
            current.document == key && current.path.as_ref() == Some(&chunk.path)
        }) && self.hunks.iter().any(|hunk| hunk == &chunk.hunk)
    }

    pub(crate) fn navigate(&mut self, path: &Path, line: usize, previous: bool) -> Option<usize> {
        let selected = self
            .chunk
            .as_ref()
            .and_then(|chunk| self.hunks.iter().position(|h| h == &chunk.hunk));
        let start = |h: &Hunk| h.changes.first().map_or(0, |change| change.lines.start);
        let count = self.hunks.len();
        if count == 0 {
            return None;
        }
        let index = match selected {
            Some(index) if previous => (index + count - 1) % count,
            Some(index) => (index + 1) % count,
            None if previous => self
                .hunks
                .iter()
                .rposition(|h| start(h) < line)
                .unwrap_or(count - 1),
            None => self.hunks.iter().position(|h| start(h) > line).unwrap_or(0),
        };
        let line = start(&self.hunks[index]);
        self.open_chunk(index, path);
        Some(line)
    }

    pub(crate) fn snapshot_fixture(
        root: &Path,
        path: &Path,
        source: &str,
        open_chunk: bool,
    ) -> Self {
        let lines = source.lines().collect::<Vec<_>>();
        let mut hunks = Vec::new();
        for (line, kind) in [
            (2, ChangeKind::Modified),
            (7, ChangeKind::Added),
            (12, ChangeKind::Deleted),
        ] {
            if let Some(text) = lines.get(line) {
                hunks.push(Hunk {
                    text: format!(
                        "@@ -{0},1 +{0},1 @@\n-{1}\n+{2}\n",
                        line + 1,
                        "The previous version of this line.",
                        text
                    ),
                    changes: vec![LineChange {
                        lines: line..line + usize::from(kind != ChangeKind::Deleted),
                        kind,
                        old_line_count: usize::from(kind != ChangeKind::Added),
                        new_line_count: usize::from(kind != ChangeKind::Deleted),
                    }],
                });
            }
        }
        let snapshot = Snapshot {
            root: root.into(),
            entries: vec![
                Entry {
                    path: path.strip_prefix(root).unwrap_or(path).into(),
                    index: 'M',
                    worktree: 'M',
                },
                Entry {
                    path: "references.bib".into(),
                    index: 'A',
                    worktree: ' ',
                },
            ]
            .into(),
            ..Default::default()
        };
        let mut state = Self {
            repository: Some(root.into()),
            statuses: FileStatuses::from_snapshot(&snapshot),
            hunks,
            ..Default::default()
        };
        if open_chunk {
            state.open_chunk(0, path);
        }
        state
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MarkerGeometry {
    pub(crate) paint: egui::Rect,
    pub(crate) hit: egui::Rect,
}

/// Logical row rectangles include every wrapped visual row. Deletions sit at
/// a boundary and remain clickable at the beginning/end of an empty document.
pub(crate) fn marker_geometry(
    change: &LineChange,
    rows: &[egui::Rect],
    gutter_left: f32,
    clip: egui::Rect,
) -> Option<MarkerGeometry> {
    let first = rows.get(change.lines.start.min(rows.len().checked_sub(1)?))?;
    // Keep the hit target inside the Git lane: the adjacent space now owns
    // fold controls and clickable line numbers.
    let gutter_right = gutter_left + f32::from(GUTTER_WIDTH);
    let hit_right = gutter_left + GUTTER_HIT_WIDTH;
    let (paint, hit) = if change.lines.is_empty() {
        if first.height() == 0.0 {
            return None;
        }
        let y = if change.lines.start >= rows.len() {
            first.bottom()
        } else {
            first.top()
        };
        (
            egui::Rect::from_center_size(
                egui::pos2(gutter_left + f32::from(GUTTER_WIDTH) / 2.0, y),
                egui::vec2(f32::from(GUTTER_WIDTH), 3.0),
            ),
            egui::Rect::from_center_size(
                egui::pos2(gutter_left + GUTTER_HIT_WIDTH / 2.0, y),
                egui::vec2(GUTTER_HIT_WIDTH, 12.0),
            ),
        )
    } else {
        let last = rows.get(change.lines.end.saturating_sub(1).min(rows.len() - 1))?;
        (
            egui::Rect::from_min_max(
                egui::pos2(gutter_left, first.top()),
                egui::pos2(gutter_right, last.bottom()),
            ),
            egui::Rect::from_min_max(
                egui::pos2(gutter_left, first.top()),
                egui::pos2(hit_right, last.bottom()),
            ),
        )
    };
    let hit = hit.intersect(clip);
    hit.is_positive().then_some(MarkerGeometry {
        paint: paint.intersect(clip),
        hit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::editor::view::{Action as ViewAction, show_chunk, show_markers};
    use crate::git::repository::snapshot;
    use std::{fs, thread};

    #[test]
    fn hunk_navigation_uses_caret_then_selection_and_wraps_both_ways() {
        let path = Path::new("main.typ");
        let source = (0..20).map(|i| format!("line {i}\n")).collect::<String>();
        let mut state = GitEditorState::snapshot_fixture(Path::new("."), path, &source, false);
        assert_eq!(state.navigate(path, 3, false), Some(7));
        assert_eq!(state.navigate(path, 0, false), Some(12));
        assert_eq!(state.navigate(path, 0, false), Some(2));
        assert_eq!(state.navigate(path, 0, true), Some(12));
        state.chunk = None;
        assert_eq!(state.navigate(path, 7, true), Some(2));
        state.hunks.clear();
        assert_eq!(state.navigate(path, 0, false), None);
    }

    fn key(root: &Path, revision: u64) -> RequestKey {
        RequestKey {
            workspace: root.into(),
            path: Some(root.join("main.typ")),
            document: DocumentKey {
                owner: tiptoptyp_core::document::WindowSessionId::new(1),
                epoch: 1,
                revision,
            },
        }
    }

    pub(super) fn initialize(root: &Path, source: &str) {
        text_run(root, &["init"]).unwrap();
        for (name, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            text_run(root, &["config", name, value]).unwrap();
        }
        fs::write(root.join("main.typ"), source).unwrap();
        text_run(root, &["add", "--", "main.typ"]).unwrap();
        text_run(root, &["commit", "-m", "Initial"]).unwrap();
    }

    #[test]
    fn customized_diff_output_keeps_blank_context_lines_and_change_markers() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        initialize(&root, "first\n\nold\nlast\n");
        for (key, value) in [
            ("diff.suppressBlankEmpty", "true"),
            ("diff.outputIndicatorNew", ">"),
            ("diff.outputIndicatorOld", "<"),
            ("diff.outputIndicatorContext", "="),
        ] {
            text_run(&root, &["config", key, value]).unwrap();
        }
        let hunks = buffer_hunks(
            &snapshot(&root).unwrap(),
            Some(&root.join("main.typ")),
            "first\n\nnew\nlast\n",
        )
        .unwrap();
        assert_eq!(
            hunks[0].changes,
            [LineChange {
                lines: 2..3,
                kind: ChangeKind::Modified,
                old_line_count: 1,
                new_line_count: 1,
            }]
        );
        assert!(hunks[0].text.contains("-old\n+new\n"));
    }

    #[test]
    fn intent_to_add_files_have_added_badges_and_buffer_markers() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        initialize(&root, "initial\n");
        let path = root.join("new.typ");
        fs::write(&path, "saved\n").unwrap();
        text_run(&root, &["add", "-N", "--", "new.typ"]).unwrap();
        let status = snapshot(&root).unwrap();
        let badges = FileStatuses::from_snapshot(&status);
        assert_eq!(badges.get(&path).unwrap().letter(), "A");
        assert!(
            badges
                .get(&path)
                .unwrap()
                .description()
                .contains("unstaged")
        );
        let hunks = buffer_hunks(&status, Some(&path), "unsaved\n").unwrap();
        assert_eq!(
            hunks[0].changes,
            [LineChange {
                lines: 0..1,
                kind: ChangeKind::Added,
                old_line_count: 0,
                new_line_count: 1,
            }]
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "saved\n");
    }

    #[test]
    fn hunk_markers_exclude_context_and_group_replacements() {
        let diff = "diff --git a/main.typ b/main.typ\n@@ -1,5 +1,6 @@\n context\n-old one\n-old two\n+new one\n+new two\n+new three\n context\n final\n@@ -20 +21 @@\n-old\n+new\n\\ No newline at end of file\n";
        let hunks = parse_hunks(diff).unwrap();
        assert_eq!(hunks.len(), 2);
        assert_eq!(
            hunks[0].changes,
            [LineChange {
                lines: 1..4,
                kind: ChangeKind::Modified,
                old_line_count: 2,
                new_line_count: 3,
            }]
        );
        assert_eq!(
            hunks[1].changes,
            [LineChange {
                lines: 20..21,
                kind: ChangeKind::Modified,
                old_line_count: 1,
                new_line_count: 1,
            }]
        );
        assert!(hunks[1].text.ends_with("\\ No newline at end of file\n"));
        assert!(!hunks[0].text.contains("diff --git"));
    }

    #[test]
    fn additions_and_deletions_use_buffer_line_boundaries() {
        for (diff, lines, kind, old_line_count, new_line_count) in [
            (
                "@@ -0,0 +1,2 @@\n+one\n+two\n",
                0..2,
                ChangeKind::Added,
                0,
                2,
            ),
            (
                "@@ -1,2 +0,0 @@\n-one\n-two\n",
                0..0,
                ChangeKind::Deleted,
                2,
                0,
            ),
            (
                "@@ -8,2 +7,0 @@\n-eight\n-nine\n",
                7..7,
                ChangeKind::Deleted,
                2,
                0,
            ),
            (
                "@@ -1,3 +1,2 @@\n-first\n second\n third\n",
                0..0,
                ChangeKind::Deleted,
                1,
                0,
            ),
            (
                "@@ -1,2 +1,3 @@\n one\n+two\n three\n",
                1..2,
                ChangeKind::Added,
                0,
                1,
            ),
        ] {
            assert_eq!(
                parse_hunks(diff).unwrap()[0].changes,
                [LineChange {
                    old_line_count,
                    new_line_count,
                    lines,
                    kind,
                }]
            );
        }
        assert!(parse_hunks("@@ malformed @@\n+x\n").is_err());
    }

    #[test]
    fn line_change_counts_include_deleted_lines_at_empty_boundaries() {
        let hunks = parse_hunks(
            "@@ -1,4 +1,5 @@\n context\n-old\n+new\n+newer\n context\n@@ -8,2 +9,0 @@\n-gone\n-also gone\n@@ -12,0 +11,2 @@\n+one\n+two\n",
        )
        .unwrap();

        assert_eq!(
            LineChangeCounts::from_hunks(&hunks),
            LineChangeCounts {
                added: 3,
                modified: 1,
                deleted: 2,
            }
        );
    }

    #[test]
    fn buffer_diff_includes_unsaved_and_staged_edits_without_writing_the_document() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        initialize(&root, "first\nsecond\nthird\n");
        fs::write(root.join("main.typ"), "first\nstaged\nthird\n").unwrap();
        text_run(&root, &["add", "--", "main.typ"]).unwrap();
        let status = snapshot(&root).unwrap();
        let hunks = buffer_hunks(
            &status,
            Some(&root.join("main.typ")),
            "first\nunsaved 文稿\nthird\n",
        )
        .unwrap();
        assert_eq!(
            hunks[0].changes,
            [LineChange {
                lines: 1..2,
                kind: ChangeKind::Modified,
                old_line_count: 1,
                new_line_count: 1,
            }]
        );
        assert!(hunks[0].text.contains("-second\n+unsaved 文稿"));
        assert!(!hunks[0].text.contains("staged"));
        assert_eq!(
            fs::read_to_string(root.join("main.typ")).unwrap(),
            "first\nstaged\nthird\n"
        );
        assert_eq!(
            text_run(&root, &["show", ":main.typ"]).unwrap(),
            "first\nstaged\nthird"
        );
        assert!(
            buffer_hunks(
                &status,
                Some(&root.join("main.typ")),
                "first\nsecond\nthird\n"
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn new_unborn_and_ignored_files_have_the_expected_decorations() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        text_run(&root, &["init"]).unwrap();
        fs::write(root.join("main.typ"), "new\n").unwrap();
        let status = snapshot(&root).unwrap();
        assert_eq!(
            FileStatuses::from_snapshot(&status)
                .get(&root.join("main.typ"))
                .unwrap()
                .letter(),
            "?"
        );
        let hunks = buffer_hunks(&status, Some(&root.join("main.typ")), "new\nunsaved\n").unwrap();
        assert_eq!(
            hunks[0].changes,
            [LineChange {
                lines: 0..2,
                kind: ChangeKind::Added,
                old_line_count: 0,
                new_line_count: 2,
            }]
        );
        text_run(&root, &["add", "--", "main.typ"]).unwrap();
        let status = snapshot(&root).unwrap();
        assert_eq!(
            FileStatuses::from_snapshot(&status)
                .get(&root.join("main.typ"))
                .unwrap()
                .letter(),
            "A"
        );
        assert!(
            !buffer_hunks(&status, Some(&root.join("main.typ")), "new\n")
                .unwrap()
                .is_empty()
        );
        fs::write(root.join(".gitignore"), "ignored.typ\n").unwrap();
        fs::write(root.join("ignored.typ"), "ignored\n").unwrap();
        let status = snapshot(&root).unwrap();
        assert!(
            buffer_hunks(&status, Some(&root.join("ignored.typ")), "unsaved\n")
                .unwrap()
                .is_empty()
        );
        assert!(
            buffer_hunks(&status, Some(Path::new("/outside.typ")), "outside")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn status_badges_explain_partial_staging_and_conflicts() {
        for (index, worktree, letter, description) in [
            (' ', 'M', "M", "Modified · unstaged changes"),
            ('M', ' ', "M", "Modified · staged"),
            ('M', 'M', "M", "Modified · staged and unstaged changes"),
            ('A', ' ', "A", "Added · staged"),
            ('?', '?', "?", "Untracked · not staged"),
            ('D', ' ', "D", "Deleted · staged"),
            (
                'U',
                'U',
                "U",
                "Merge conflict · staged and unstaged changes",
            ),
        ] {
            let status = FileStatus { index, worktree };
            assert_eq!(status.letter(), letter);
            assert_eq!(status.description(), description);
        }
    }

    #[test]
    fn switching_files_detaches_obsolete_scans_but_typing_keeps_one_worker() {
        let root = Path::new("/first");
        for mut next in [key(root, 3), key(Path::new("/second"), 3)] {
            next.path = Some(next.workspace.join("other.typ"));
            let mut state = GitEditorState {
                current: Some(key(root, 1)),
                repository: Some(root.into()),
                ..Default::default()
            };
            let (release, blocked) = std::sync::mpsc::channel();
            state
                .job
                .start("blocked-git-scan", move || {
                    let _ = blocked.recv();
                    Err("obsolete failure".into())
                })
                .unwrap();
            let now = Instant::now();
            state.prepare_request(&key(root, 2), now);
            assert!(state.job.is_running(), "typing reuses the active scan");
            assert_eq!(state.scan_deadline, now + EDIT_DEBOUNCE);
            assert_eq!(state.repository.as_deref(), Some(root));

            state.prepare_request(&next, now);
            assert!(!state.job.is_running(), "a new file can scan immediately");
            assert_eq!(state.scan_deadline, now);
            assert_eq!(state.current.as_ref(), Some(&next));
            assert_eq!(state.repository.is_none(), next.workspace != root);
            release.send(()).unwrap();
            assert!(matches!(state.job.poll(), LatestJobPoll::Idle));
        }
    }

    #[test]
    fn unchanged_editor_does_not_schedule_periodic_scans() {
        let root = Path::new("/project");
        let request = key(root, 1);
        let mut state = GitEditorState {
            current: Some(request.clone()),
            repository: Some(root.into()),
            scan_deadline: Instant::now() - Duration::from_secs(10),
            refresh_requested: false,
            ..Default::default()
        };

        state.tick(
            &egui::Context::default(),
            root,
            request.path.as_deref(),
            request.document,
            "unchanged",
            None,
        );

        assert!(!state.job.is_running());
        assert!(!state.refresh_requested);
    }

    #[test]
    fn running_editor_scan_waits_for_completion_instead_of_polling_frames() {
        let root = Path::new("/project");
        let request = key(root, 1);
        let (release, blocked) = std::sync::mpsc::channel();
        let (repaint_tx, repaint_rx) = std::sync::mpsc::channel();
        let context = egui::Context::default();
        context.set_request_repaint_callback(move |info| {
            let _ = repaint_tx.send(info.viewport_id);
        });
        let mut state = GitEditorState {
            current: Some(request.clone()),
            repository: Some(root.into()),
            refresh_requested: false,
            ..Default::default()
        };
        state
            .job
            .start("blocked-editor-scan", move || {
                blocked.recv().unwrap();
                Err("finished".into())
            })
            .unwrap();

        state.tick(
            &context,
            root,
            request.path.as_deref(),
            request.document,
            "unchanged",
            None,
        );

        assert!(repaint_rx.try_recv().is_err());
        release.send(()).unwrap();
    }

    #[test]
    fn late_results_cannot_replace_a_new_file_revision_or_another_workspace() {
        let root = Path::new("/first");
        let mut state = GitEditorState {
            current: Some(key(root, 2)),
            repository: Some(root.into()),
            ..Default::default()
        };
        let hunk = Hunk {
            text: "obsolete".into(),
            changes: Vec::new(),
        };
        for old_key in [key(root, 1), key(Path::new("/second"), 2)] {
            state.accept(ScanResult {
                key: old_key,
                repository: Some(root.into()),
                status: FileStatuses::default(),
                hunks: Ok(vec![hunk.clone()]),
            });
            assert!(state.hunks.is_empty());
        }
        state.accept(ScanResult {
            key: key(root, 2),
            repository: Some(root.into()),
            status: FileStatuses::default(),
            hunks: Ok(vec![hunk]),
        });
        assert_eq!(state.hunks.len(), 1);
        state.open_chunk(0, &root.join("main.typ"));
        state.hunks.clear();
        assert!(
            state.has_gutter(Some(&root.join("main.typ"))),
            "gutter width must remain stable during edits"
        );
        assert_eq!(
            state.chunk.unwrap().hunk.text,
            "obsolete",
            "opened chunks own their contents"
        );
    }

    #[test]
    fn changing_the_document_revision_drops_a_selected_chunk() {
        let root = Path::new("/project");
        let mut state = GitEditorState {
            current: Some(key(root, 1)),
            chunk: Some(ChunkDiff {
                path: root.join("main.typ"),
                hunk: Hunk {
                    text: "-old\n+new\n".into(),
                    changes: Vec::new(),
                },
            }),
            ..Default::default()
        };

        state.prepare_request(&key(root, 2), Instant::now());

        assert!(state.chunk.is_none());
    }

    #[test]
    fn editing_keeps_stale_hunks_until_the_fresh_scan_is_accepted() {
        let root = Path::new("/project");
        let stale = Hunk {
            text: "stale".into(),
            changes: vec![LineChange {
                lines: 1..2,
                kind: ChangeKind::Modified,
                old_line_count: 1,
                new_line_count: 1,
            }],
        };
        let fresh = Hunk {
            text: "fresh".into(),
            changes: vec![LineChange {
                lines: 3..4,
                kind: ChangeKind::Added,
                old_line_count: 0,
                new_line_count: 1,
            }],
        };
        let mut state = GitEditorState {
            current: Some(key(root, 1)),
            repository: Some(root.into()),
            hunks: vec![stale.clone()],
            ..Default::default()
        };

        state.prepare_request(&key(root, 2), Instant::now());
        assert_eq!(state.hunks, [stale]);

        state.accept(ScanResult {
            key: key(root, 2),
            repository: Some(root.into()),
            status: FileStatuses::default(),
            hunks: Ok(vec![fresh.clone()]),
        });
        assert_eq!(state.hunks, [fresh]);

        let replacement = RequestKey {
            document: DocumentKey {
                epoch: 2,
                revision: 0,
                ..key(root, 2).document
            },
            ..key(root, 2)
        };
        state.prepare_request(&replacement, Instant::now());
        assert!(state.hunks.is_empty());
    }

    #[test]
    fn windows_keep_independent_buffers_statuses_and_selected_chunks() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let roots = [
            a.path().canonicalize().unwrap(),
            b.path().canonicalize().unwrap(),
        ];
        for root in &roots {
            initialize(root, "original\n");
        }
        fs::write(roots[0].join("main.typ"), "disk edit\n").unwrap();
        let context = egui::Context::default();
        let mut states = [
            GitEditorState::default(),
            GitEditorState::default(),
            GitEditorState::default(),
        ];
        // Two windows even share a repository/file, but keep different buffers.
        let cases = [
            (&roots[0], "window one\n"),
            (&roots[1], "window two\n"),
            (&roots[0], "window three\n"),
        ];
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            for (state, (root, buffer)) in states.iter_mut().zip(cases) {
                let request = key(root, 1);
                state.tick(
                    &context,
                    root,
                    request.path.as_deref(),
                    request.document,
                    buffer,
                    None,
                );
            }
            if states.iter().all(|state| !state.hunks.is_empty()) {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        for (state, (root, buffer)) in states.iter_mut().zip(cases) {
            assert!(state.hunks[0].text.contains(&format!("+{buffer}")));
            state.open_chunk(0, &root.join("main.typ"));
        }
        assert_eq!(
            states[0]
                .statuses
                .get(&roots[0].join("main.typ"))
                .unwrap()
                .letter(),
            "M"
        );
        assert!(states[1].statuses.get(&roots[0].join("main.typ")).is_none());
        assert!(
            states[1].statuses.get(&roots[1].join("main.typ")).is_none(),
            "clean disk is not marked by another window's unsaved buffer"
        );
        states[0].chunk = None;
        assert!(
            states[1]
                .chunk
                .as_ref()
                .unwrap()
                .hunk
                .text
                .contains("+window two")
        );
        assert!(
            states[2]
                .chunk
                .as_ref()
                .unwrap()
                .hunk
                .text
                .contains("+window three")
        );
    }

    #[test]
    fn marker_geometry_covers_wrapped_rows_and_clips_deletion_hit_targets() {
        let rows = [
            egui::Rect::from_min_max(egui::pos2(40.0, 0.0), egui::pos2(400.0, 20.0)),
            egui::Rect::from_min_max(egui::pos2(40.0, 20.0), egui::pos2(400.0, 80.0)),
            egui::Rect::from_min_max(egui::pos2(40.0, 80.0), egui::pos2(400.0, 100.0)),
        ];
        let clip = egui::Rect::from_min_max(egui::pos2(0.0, 10.0), egui::pos2(500.0, 90.0));
        let modified = marker_geometry(
            &LineChange {
                lines: 1..2,
                kind: ChangeKind::Modified,
                old_line_count: 1,
                new_line_count: 1,
            },
            &rows,
            0.0,
            clip,
        )
        .unwrap();
        assert_eq!(modified.paint.y_range(), egui::Rangef::new(20.0, 80.0));
        assert!(modified.hit.right() < rows[0].left());
        for boundary in [0, 3] {
            assert!(
                marker_geometry(
                    &LineChange {
                        lines: boundary..boundary,
                        kind: ChangeKind::Deleted,
                        old_line_count: 1,
                        new_line_count: 0,
                    },
                    &rows,
                    0.0,
                    clip
                )
                .is_none()
            );
        }
        let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(500.0, 100.0));
        for boundary in [0, 3] {
            let marker = marker_geometry(
                &LineChange {
                    lines: boundary..boundary,
                    kind: ChangeKind::Deleted,
                    old_line_count: 1,
                    new_line_count: 0,
                },
                &rows,
                0.0,
                full,
            )
            .unwrap();
            assert!(full.contains_rect(marker.hit));
            assert!(marker.hit.height() >= 6.0);
        }
    }

    #[test]
    fn marker_paint_fills_the_compact_lane_without_shrinking_its_hit_target() {
        let rows = [egui::Rect::from_min_max(
            egui::pos2(40.0, 0.0),
            egui::pos2(400.0, 20.0),
        )];
        let clip = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(500.0, 100.0));
        let gutter_left = 100.0;
        let gutter_right = gutter_left + f32::from(GUTTER_WIDTH);
        let hit_right = gutter_left + GUTTER_HIT_WIDTH;

        for change in [
            LineChange {
                lines: 0..1,
                kind: ChangeKind::Modified,
                old_line_count: 1,
                new_line_count: 1,
            },
            LineChange {
                lines: 1..1,
                kind: ChangeKind::Deleted,
                old_line_count: 1,
                new_line_count: 0,
            },
        ] {
            let geometry = marker_geometry(&change, &rows, gutter_left, clip).unwrap();
            assert_eq!(geometry.paint.min.x, gutter_left);
            assert_eq!(geometry.paint.max.x, gutter_right);
            assert_eq!(geometry.hit.min.x, gutter_left);
            assert_eq!(geometry.hit.max.x, hit_right);
        }
    }

    #[test]
    fn clicking_a_marker_opens_only_its_chunk() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let hunks =
            parse_hunks("@@ -1 +1 @@\n-old\n+first edit\n@@ -3 +3 @@\n-old\n+second edit\n")
                .unwrap();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(620.0, 420.0))
            .build_ui_state(
                move |ui, selected| {
                    let rows = (0..3)
                        .map(|line| {
                            egui::Rect::from_min_size(
                                egui::pos2(30.0, 20.0 + line as f32 * 20.0),
                                egui::vec2(500.0, 20.0),
                            )
                        })
                        .collect::<Vec<_>>();
                    if let Some(ViewAction::OpenChunk(index)) = show_markers(ui, &hunks, &rows, 8.0)
                    {
                        *selected = Some(index);
                    }
                    ui.add_space(90.0);
                    if let Some(index) = selected {
                        show_chunk(
                            ui,
                            &ChunkDiff {
                                path: "/project/main.typ".into(),
                                hunk: hunks[*index].clone(),
                            },
                            &crate::shortcuts::ShortcutBindings::defaults(
                                crate::shortcuts::ShortcutPlatform::current(),
                            ),
                            false,
                            crate::settings::GitDiffStyle::Unified,
                            crate::settings::ToolbarStyle::Icons,
                        );
                    }
                },
                None::<usize>,
            );
        harness.run();
        harness
            .get_by_label("Git change: modified lines 3–3")
            .click();
        harness.run();
        assert_eq!(*harness.state(), Some(1));
        harness.get_by_label_contains("+second edit");
        assert!(harness.query_by_label_contains("+first edit").is_none());
    }
}
