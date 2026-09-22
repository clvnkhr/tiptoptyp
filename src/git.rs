//! Basic Git operations. Commands run off the UI thread, with literal paths
//! and no terminal prompts; every result carries its originating workspace.
pub(crate) mod editor;
use crate::settings::GitDiffStyle;
use crate::worker::{ExclusiveJob, LatestJobPoll};
use eframe::egui;
#[cfg(test)]
use repository::{
    ChangeList, args, parse_status, path_from_bytes, perform, private_artifact, run_command,
    snapshot, status_snapshot, text_run,
};
use repository::{
    DiffKind, DiffResult, DiffSelection, Entry, Operation, Repository, ResultData, Snapshot,
};
#[cfg(test)]
use std::fs;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
pub(crate) mod repository;
pub(crate) mod view;

impl DiffKind {
    fn description(self) -> &'static str {
        match self {
            Self::WorkingTree => {
                "Working file compared with the staging area. New files show their full contents."
            }
            Self::Staged => {
                "Staging area compared with the last commit. These changes will go into the next commit."
            }
        }
    }
    fn empty_message(self) -> &'static str {
        match self {
            Self::WorkingTree => {
                "No unstaged changes in this file. Use Staged diff to see changes already staged."
            }
            Self::Staged => {
                "No staged changes in this file. Stage your changes to include them in the next commit."
            }
        }
    }
}

#[derive(Debug)]
struct DiffView {
    selection: DiffSelection,
    // None means the worker is still loading the comparison.
    content: Option<Result<DiffContent, String>>,
    reveal: bool,
}

#[derive(Debug)]
struct DiffContent {
    text: Arc<str>,
}

impl From<String> for DiffContent {
    fn from(text: String) -> Self {
        Self { text: text.into() }
    }
}

impl crate::worker::OperationSummary for ResultData {
    fn completion_summary(&self) -> String {
        format!("{}: {}", self.workspace.display(), self.output)
    }
}

pub(crate) struct GitPanel {
    pub(crate) visible: bool,
    workspace: PathBuf,
    snapshot: Snapshot,
    job: ExclusiveJob<ResultData>,
    job_workspace: Option<PathBuf>,
    message: String,
    commit_message: String,
    failed: bool,
    diff: Option<DiffView>,
    refresh_requested: bool,
    pending_operation: Option<Operation>,
    background_refresh: bool,
    status_changed: bool,
    view_cache: view::Cache,
    #[cfg(test)]
    rendered_change_rows: usize,
}

impl Default for GitPanel {
    fn default() -> Self {
        Self {
            // Git is a first-class Explorer section. View > Git can still
            // hide it when users want more room for the file tree.
            visible: true,
            workspace: PathBuf::new(),
            snapshot: Snapshot::default(),
            job: ExclusiveJob::default(),
            job_workspace: None,
            message: String::new(),
            commit_message: String::new(),
            failed: false,
            diff: None,
            refresh_requested: false,
            pending_operation: None,
            background_refresh: false,
            status_changed: false,
            view_cache: view::Cache::default(),
            #[cfg(test)]
            rendered_change_rows: 0,
        }
    }
}

impl GitPanel {
    /// Queue a status scan for the next panel frame. The request is retained
    /// while a Git operation is running, so filesystem updates cannot be lost
    /// behind an in-flight stage, commit, or diff operation.
    pub(crate) fn request_refresh(&mut self) {
        self.refresh_requested = true;
    }

    /// Report a completed Git operation that changed the repository snapshot.
    /// The document editor consumes this signal to refresh its gutter without
    /// polling for commits or staging changes.
    pub(crate) fn take_status_changed(&mut self) -> bool {
        std::mem::take(&mut self.status_changed)
    }

    pub(crate) fn open(&mut self, context: &egui::Context, workspace: &Path) {
        self.visible = true;
        self.sync_workspace(context, workspace);
        if !self.job.is_running() {
            self.start(context, Operation::Refresh);
        }
    }

    fn sync_workspace(&mut self, context: &egui::Context, workspace: &Path) {
        if self.workspace != workspace {
            self.workspace = workspace.to_path_buf();
            self.snapshot = Snapshot::default();
            self.commit_message.clear();
            self.message.clear();
            self.diff = None;
            self.pending_operation = None;
            if !self.job.is_running() {
                self.start_operation(context, Operation::Refresh, true);
            } else {
                self.refresh_requested = true;
            }
        }
    }

    fn start(&mut self, context: &egui::Context, operation: Operation) {
        self.start_operation(context, operation, false);
    }

    fn start_operation(&mut self, context: &egui::Context, operation: Operation, quiet: bool) {
        if self.job.is_running() {
            if matches!(&operation, Operation::Refresh) {
                self.refresh_requested = true;
            } else if self.background_refresh {
                // Background status reads leave the controls responsive. Hold
                // the user's action until the read completes rather than
                // silently dropping a click during that short interval.
                self.pending_operation = Some(operation);
            }
            return;
        }
        if matches!(&operation, Operation::Refresh) {
            self.refresh_requested = false;
        }
        if let Operation::Diff(path, kind) = &operation
            && self
                .diff
                .as_ref()
                .is_some_and(|diff| diff.selection.path == *path && diff.selection.kind == *kind)
        {
            // The row action doubles as the toggle. This keeps the diff page
            // focused on the comparison itself without a redundant close
            // button in its header.
            self.diff = None;
            return;
        }
        let selection = match &operation {
            Operation::Diff(path, kind) => Some(DiffSelection {
                path: path.clone(),
                kind: *kind,
            }),
            _ => None,
        };
        self.diff = selection.clone().map(|selection| DiffView {
            selection,
            content: None,
            reveal: true,
        });
        self.background_refresh = quiet && matches!(&operation, Operation::Refresh);
        let workspace = self.workspace.clone();
        self.job_workspace = Some(workspace.clone());
        self.failed = false;
        if !self.background_refresh {
            self.message = "Working…".into();
        }
        if let Err(error) = self.job.start_and_repaint("git", context, move || {
            // Return operation errors as data too, so a late failure cannot
            // overwrite another workspace's state.
            let result = Repository::new(&workspace).execute(operation);
            Ok(match result {
                Ok(result) => result,
                Err(error) => ResultData {
                    workspace,
                    snapshot: Snapshot::default(),
                    output: format!("Error: {error}"),
                    committed: false,
                    failed: true,
                    diff: selection.map(|selection| DiffResult {
                        selection,
                        content: Err(error),
                    }),
                },
            })
        }) {
            self.failed = true;
            self.background_refresh = false;
            self.message = error.clone();
            if let Some(diff) = &mut self.diff {
                diff.content = Some(Err(error));
            }
        }
    }

    pub(crate) fn poll(&mut self, context: &egui::Context, workspace: &Path) {
        if !self.visible && !self.job.is_running() {
            return;
        }
        self.sync_workspace(context, workspace);
        match self.job.poll() {
            LatestJobPoll::Ready(result) if result.workspace == self.workspace => {
                let quiet_refresh = self.background_refresh;
                let was_failed = self.failed;
                let snapshot_changed = self.snapshot != result.snapshot;
                self.status_changed |= snapshot_changed;
                self.background_refresh = false;
                self.failed = result.failed;
                if !self.failed {
                    self.snapshot = result.snapshot;
                }
                if let Some(result) = result.diff
                    && let Some(view) = &mut self.diff
                    && view.selection == result.selection
                {
                    view.content = Some(result.content.map(DiffContent::from));
                    // The completed diff can be taller than the loading card.
                    view.reveal = true;
                }
                if !quiet_refresh || snapshot_changed || was_failed != self.failed {
                    self.message = result.output;
                }
                if result.committed {
                    self.commit_message.clear();
                }
            }
            LatestJobPoll::Ready(_) => self.refresh_requested = true,
            LatestJobPoll::Failed(_) if self.job_workspace.as_ref() != Some(&self.workspace) => {
                self.background_refresh = false;
                self.refresh_requested = true;
            }
            LatestJobPoll::Failed(error) => {
                let quiet_refresh = self.background_refresh;
                let error_changed = !self.failed || self.message != error;
                self.background_refresh = false;
                self.failed = true;
                if !quiet_refresh || error_changed {
                    self.message = error.clone();
                }
                if let Some(diff) = &mut self.diff {
                    diff.content = Some(Err(error));
                }
            }
            LatestJobPoll::Pending | LatestJobPoll::Idle => {}
        }
        if !self.job.is_running() {
            if let Some(operation) = self.pending_operation.take() {
                self.start_operation(context, operation, false);
            } else if self.refresh_requested {
                self.refresh_requested = false;
                self.start_operation(context, Operation::Refresh, true);
            }
        }
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        dirty: bool,
        diff_style: GitDiffStyle,
    ) -> view::Output {
        // The app update loop polls independently of whether this body is open.
        // The view receives immutable model state and cannot start repository IO.
        // Maintenance scans stay out of the visible loading state. They must
        // not dim controls or replace the stable status row on every timer
        // tick; explicit operations still use the normal busy state.
        let busy =
            (self.job.is_running() && !self.background_refresh) || self.pending_operation.is_some();
        let output = view::show_panel(
            ui,
            view::Input {
                snapshot: &self.snapshot,
                message: &self.message,
                commit_message: &self.commit_message,
                failed: self.failed,
                diff: self.diff.as_ref(),
                diff_style,
                busy,
                dirty,
            },
            &mut self.view_cache,
        );
        #[cfg(test)]
        {
            self.rendered_change_rows = output.rendered_change_rows;
        }
        output
    }

    pub(crate) fn apply_view_output(&mut self, context: &egui::Context, output: view::Output) {
        if let Some(message) = output.commit_message {
            self.commit_message = message;
        }
        if let Some(selection) = output.revealed
            && let Some(diff) = &mut self.diff
            && diff.selection == selection
        {
            diff.reveal = false;
        }
        if let Some(operation) = output.operation {
            self.start(context, operation);
            context.request_repaint();
        }
    }

    pub(crate) fn snapshot_fixture() -> Self {
        Self {
            visible: true,
            workspace: "/Projects/research-paper".into(),
            snapshot: Snapshot {
                root: "/Projects/research-paper".into(),
                branch: "main…origin/main · 1 ahead".into(),
                entries: vec![
                    Entry {
                        path: "main.typ".into(),
                        index: 'M',
                        worktree: 'M',
                    },
                    Entry {
                        path: "chapters/experiments/supplementary-results.typ".into(),
                        index: ' ',
                        worktree: 'M',
                    },
                    Entry {
                        path: "references.bib".into(),
                        index: 'A',
                        worktree: ' ',
                    },
                    Entry {
                        path: "figures/accuracy.svg".into(),
                        index: '?',
                        worktree: '?',
                    },
                ]
                .into(),
                history: "8e041ab Refine the introduction
4b6c205 Add experiment figures"
                    .into(),
                initialized: true,
            },
            message: "Status refreshed".into(),
            commit_message: "Update the evaluation results".into(),
            diff: Some(DiffView {
                selection: DiffSelection {
                    path: "main.typ".into(),
                    kind: DiffKind::WorkingTree,
                },
                content: Some(Ok("diff --git a/main.typ b/main.typ
--- a/main.typ
+++ b/main.typ
@@ -12,3 +12,3 @@
 = Evaluation
-The baseline reaches 91% accuracy.
+The revised model reaches 96% accuracy.
 See @fig-results for the full comparison.
"
                .to_owned()
                .into())),
                reveal: false,
            }),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn show_panel(ui: &mut egui::Ui, panel: &mut GitPanel) {
        let output = panel.show(ui, false, crate::settings::GitDiffStyle::Unified);
        panel.apply_view_output(ui.ctx(), output);
    }

    #[test]
    fn typing_with_an_open_git_panel_only_builds_visible_change_rows() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut panel = GitPanel::snapshot_fixture();
        panel.diff = None;
        panel.snapshot.entries = (0..10_000)
            .map(|index| Entry {
                path: format!("source/file-{index:05}.typ").into(),
                index: ' ',
                worktree: 'M',
            })
            .collect::<Vec<_>>()
            .into();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 600.0))
            .build_ui_state(
                |ui, (panel, source)| {
                    egui::Panel::left("git-test-panel")
                        .exact_size(560.0)
                        .show(ui, |ui| {
                            show_panel(ui, panel);
                        });
                    egui::CentralPanel::default().show(ui, |ui| {
                        let label = ui.label("Source editor");
                        ui.add(egui::TextEdit::multiline(source))
                            .labelled_by(label.id);
                    });
                },
                (panel, String::new()),
            );
        harness.run();
        assert!(
            harness.state().0.rendered_change_rows <= 10,
            "built {} rows while only a handful fit on screen",
            harness.state().0.rendered_change_rows
        );
        harness.get_by_label("Source editor").click();
        harness
            .get_by_label("Source editor")
            .type_text("= Fast editing");
        harness.run();
        assert_eq!(harness.state().1, "= Fast editing");
        assert!(harness.state().0.rendered_change_rows <= 10);
        assert!(!harness.state().0.job.is_running());
        assert!(!harness.state().0.refresh_requested);

        harness.get_by_label("source/file-00002.typ").hover();
        harness.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -380.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        // Allow the wheel animation to finish before targeting a virtual row.
        harness.run_steps(30);
        assert!(harness.state().0.rendered_change_rows <= 10);
        assert!(harness.query_by_label("source/file-00000.typ").is_none());
        let target = harness.get_by_label("source/file-00012.typ").rect();
        let (sender, receiver) = std::sync::mpsc::channel();
        harness.state_mut().0.background_refresh = true;
        harness
            .state_mut()
            .0
            .job
            .start_and_repaint(
                "hold-status-for-action-test",
                &egui::Context::default(),
                move || {
                    let _ = receiver.recv();
                    Err("Test finished".into())
                },
            )
            .unwrap();
        harness
            .get_all_by_label("Diff")
            .find(|button| (button.rect().center().y - target.center().y).abs() < 1.0)
            .expect("diff action beside the scrolled file")
            .click();
        harness.run_steps(2);
        assert!(matches!(&harness.state().0.pending_operation,
            Some(Operation::Diff(path, DiffKind::WorkingTree))
                if path == Path::new("source/file-00012.typ")));
        sender.send(()).unwrap();
    }

    #[test]
    fn change_summary_covers_offscreen_and_private_files() {
        let changes: ChangeList = vec![
            Entry {
                path: "clean-staged.typ".into(),
                index: 'A',
                worktree: ' ',
            },
            Entry {
                path: "partial.typ".into(),
                index: 'M',
                worktree: 'M',
            },
            Entry {
                path: "new.typ".into(),
                index: '?',
                worktree: '?',
            },
            Entry {
                path: ".tiptoptyp/tracked.typ".into(),
                index: 'A',
                worktree: 'M',
            },
        ]
        .into();
        assert_eq!(changes.staged, 3);
        assert!(changes.stageable);
        assert!(changes.staged_private);
        let private_only: ChangeList = vec![changes[3].clone()].into();
        assert!(!private_only.stageable);
        assert_eq!(private_only.staged, 1);
        assert!(private_only.staged_private);
        let empty: ChangeList = Vec::new().into();
        assert_eq!(empty.staged, 0);
        assert!(!empty.stageable);
        assert!(!empty.staged_private);
    }

    #[test]
    fn diff_layout_is_reused_until_text_style_or_font_cache_changes() {
        let context = egui::Context::default();
        let mut cache = view::Cache::default();
        let mut diff: DiffContent = "+new\n-old\n@@ hunk @@\n context\n".repeat(1000).into();
        let draw = |diff: &DiffContent, cache: &mut view::Cache| {
            let mut galley = None;
            context
                .run_ui(Default::default(), |ui| {
                    galley = Some(cache.diff_galley(ui, &diff.text, ui.available_width().max(1.0)));
                })
                .drop_without_applying_deltas();
            galley.unwrap()
        };
        let first = draw(&diff, &mut cache);
        for _ in 0..3 {
            assert!(Arc::ptr_eq(&first, &draw(&diff, &mut cache)));
        }
        context.set_visuals(egui::Visuals::light());
        let light = draw(&diff, &mut cache);
        assert!(!Arc::ptr_eq(&first, &light));
        assert!(Arc::ptr_eq(&light, &draw(&diff, &mut cache)));
        theme::configure_editor_fonts(
            &context,
            theme::FontRequest::default(),
            theme::FontRequest::default(),
            false,
            theme::FONT_WEIGHT_NORMAL,
            theme::FONT_WEIGHT_NORMAL,
            None,
        );
        let new_fonts = draw(&diff, &mut cache);
        assert!(!Arc::ptr_eq(&light, &new_fonts));
        assert!(Arc::ptr_eq(&new_fonts, &draw(&diff, &mut cache)));
        context.set_pixels_per_point(2.0);
        let scaled = draw(&diff, &mut cache);
        assert!(!Arc::ptr_eq(&new_fonts, &scaled));
        diff = "+replacement\n".to_owned().into();
        assert_eq!(draw(&diff, &mut cache).text(), "+replacement\n");
    }

    #[test]
    fn decoration_status_includes_branch_without_history_or_index_refresh() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        perform(root, Operation::Init).unwrap();
        configure_identity(root);
        fs::write(root.join("main.typ"), "initial\n").unwrap();
        perform(root, Operation::StageAll).unwrap();
        perform(root, Operation::Commit("Initial".into())).unwrap();
        let index = fs::read(root.join(".git/index")).unwrap();
        fs::write(root.join("main.typ"), "changed\n").unwrap();
        let status = status_snapshot(root).unwrap();
        assert!(status.history.is_empty());
        assert_eq!(
            status.branch,
            text_run(root, &["symbolic-ref", "--short", "HEAD"]).unwrap()
        );
        assert_eq!(status.entries.len(), 1);
        assert_eq!(status.entries[0].worktree, 'M');
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        assert!(snapshot(root).unwrap().history.contains("Initial"));
    }

    #[test]
    fn missing_workspace_reports_a_specific_git_error() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("deleted-workspace");
        let error = run_command(&missing, &args(&["status"]), false).unwrap_err();
        assert!(
            error.starts_with("Git workspace is unavailable:"),
            "unexpected error: {error}"
        );
    }

    fn configure_identity(root: &Path) {
        text_run(root, &["config", "user.name", "Test"]).unwrap();
        text_run(root, &["config", "user.email", "test@example.invalid"]).unwrap();
        text_run(root, &["config", "commit.gpgsign", "false"]).unwrap();
    }

    #[test]
    fn nested_workspace_commit_completion_is_applied_without_an_extra_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        perform(&root, Operation::Init).unwrap();
        configure_identity(&root);
        let workspace = root.join("nested");
        fs::create_dir(&workspace).unwrap();
        fs::write(root.join("new.typ"), "new\n").unwrap();
        Repository::new(&workspace)
            .execute(Operation::StageAll)
            .unwrap();
        let context = egui::Context::default();
        let mut panel = GitPanel {
            workspace: workspace.clone(),
            commit_message: "Nested commit".into(),
            ..Default::default()
        };
        panel.start(&context, Operation::Commit(panel.commit_message.clone()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while panel.job.is_running() {
            assert!(Instant::now() < deadline, "commit did not finish");
            panel.poll(&context, &workspace);
            thread::yield_now();
        }
        assert!(!panel.failed, "{}", panel.message);
        assert_eq!(panel.snapshot.root, root);
        assert!(panel.snapshot.history.contains("Nested commit"));
        assert!(panel.commit_message.is_empty());
        assert!(!panel.refresh_requested);
        assert!(panel.take_status_changed());
    }

    #[test]
    fn panel_is_visible_by_default_and_refresh_requests_pick_up_file_changes() {
        use egui_kittest::Harness;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        perform(root, Operation::Init).unwrap();
        configure_identity(root);
        fs::write(root.join("main.typ"), "initial\n").unwrap();
        perform(root, Operation::StageAll).unwrap();
        perform(root, Operation::Commit("Initial".into())).unwrap();

        let mut panel = GitPanel {
            snapshot: snapshot(root).unwrap(),
            ..Default::default()
        };
        assert!(panel.visible);
        panel.request_refresh();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(860.0, 500.0))
            .build_ui_state(
                |ui, panel| {
                    panel.poll(ui.ctx(), root);
                    show_panel(ui, panel);
                },
                panel,
            );
        harness.step();
        finish_ui_job(&mut harness);
        assert!(harness.state().snapshot.entries.is_empty());
        assert!(harness.state_mut().take_status_changed());
        assert!(!harness.state_mut().take_status_changed());

        fs::write(root.join("main.typ"), "changed\n").unwrap();
        harness.state_mut().request_refresh();
        harness.step();
        finish_ui_job(&mut harness);
        assert_eq!(
            harness
                .state()
                .snapshot
                .entries
                .iter()
                .find(|entry| entry.path == Path::new("main.typ"))
                .map(|entry| entry.worktree),
            Some('M')
        );
        assert!(harness.state_mut().take_status_changed());
    }

    #[test]
    fn background_refresh_keeps_a_stable_status_message_when_nothing_changed() {
        use egui_kittest::Harness;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        perform(root, Operation::Init).unwrap();
        configure_identity(root);
        fs::write(root.join("main.typ"), "initial\n").unwrap();
        perform(root, Operation::StageAll).unwrap();
        perform(root, Operation::Commit("Initial".into())).unwrap();

        let mut panel = GitPanel {
            workspace: root.into(),
            snapshot: snapshot(root).unwrap(),
            message: "Status refreshed".into(),
            ..Default::default()
        };
        panel.request_refresh();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(860.0, 500.0))
            .build_ui_state(
                |ui, panel| {
                    panel.poll(ui.ctx(), root);
                    show_panel(ui, panel);
                },
                panel,
            );
        harness.step();
        finish_ui_job(&mut harness);
        assert_eq!(harness.state().message, "Status refreshed");
    }

    #[test]
    fn collapsed_panel_poll_collects_background_status() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        perform(root, Operation::Init).unwrap();
        fs::write(root.join("main.typ"), "new\n").unwrap();
        let mut panel = GitPanel {
            workspace: root.into(),
            ..Default::default()
        };
        let context = egui::Context::default();
        panel.request_refresh();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            panel.poll(&context, root);
            if !panel.job.is_running() && !panel.refresh_requested {
                break;
            }
            assert!(Instant::now() < deadline, "Git panel poll timed out");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(panel.snapshot.initialized);
        assert_eq!(panel.snapshot.entries[0].path, Path::new("main.typ"));
    }

    #[test]
    fn user_action_is_queued_during_a_background_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (release, blocked) = std::sync::mpsc::channel();
        let workspace = root.to_path_buf();
        let mut panel = GitPanel {
            workspace: workspace.clone(),
            background_refresh: true,
            ..Default::default()
        };
        panel
            .job
            .start_and_repaint(
                "blocked-git-refresh",
                &egui::Context::default(),
                move || {
                    blocked.recv().unwrap();
                    Ok(ResultData {
                        workspace,
                        snapshot: Snapshot::default(),
                        output: "Status refreshed".into(),
                        committed: false,
                        failed: false,
                        diff: None,
                    })
                },
            )
            .unwrap();

        panel.start(&egui::Context::default(), Operation::Init);
        assert!(matches!(panel.pending_operation, Some(Operation::Init)));
        release.send(()).unwrap();

        let context = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            panel.poll(&context, root);
            if root.join(".git").is_dir() && !panel.job.is_running() {
                break;
            }
            assert!(Instant::now() < deadline, "queued Git action timed out");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(panel.pending_operation.is_none());
    }

    #[test]
    fn local_remote_supports_push_fetch_and_fast_forward_pull() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let remote = temp.path().join("remote.git");
        let peer = temp.path().join("peer");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&remote).unwrap();
        text_run(&remote, &["init", "--bare"]).unwrap();
        perform(&root, Operation::Init).unwrap();
        configure_identity(&root);
        fs::write(root.join("main.typ"), "first\n").unwrap();
        perform(&root, Operation::StageAll).unwrap();
        perform(&root, Operation::Commit("Initial".into())).unwrap();
        let branch = text_run(&root, &["symbolic-ref", "--short", "HEAD"]).unwrap();
        let reference = format!("refs/heads/{branch}");
        text_run(&remote, &["symbolic-ref", "HEAD", &reference]).unwrap();
        text_run(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        text_run(&root, &["config", "push.default", "current"]).unwrap();
        perform(&root, Operation::Push).unwrap();
        text_run(
            &root,
            &["branch", "--set-upstream-to", &format!("origin/{branch}")],
        )
        .unwrap();
        text_run(
            temp.path(),
            &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
        )
        .unwrap();
        configure_identity(&peer);
        fs::write(peer.join("main.typ"), "from peer\n").unwrap();
        perform(&peer, Operation::StageAll).unwrap();
        perform(&peer, Operation::Commit("Peer edit".into())).unwrap();
        perform(&peer, Operation::Push).unwrap();
        perform(&root, Operation::Fetch).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("main.typ")).unwrap(),
            "first\n"
        );
        perform(&root, Operation::Pull).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("main.typ")).unwrap(),
            "from peer\n"
        );

        fs::write(root.join("main.typ"), "local divergence\n").unwrap();
        perform(&root, Operation::StageAll).unwrap();
        perform(&root, Operation::Commit("Local edit".into())).unwrap();
        fs::write(peer.join("main.typ"), "remote divergence\n").unwrap();
        perform(&peer, Operation::StageAll).unwrap();
        perform(&peer, Operation::Commit("Remote edit".into())).unwrap();
        perform(&peer, Operation::Push).unwrap();
        assert!(perform(&root, Operation::Pull).is_err());
        assert_eq!(
            fs::read_to_string(root.join("main.typ")).unwrap(),
            "local divergence\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn repository_path_preserves_non_utf8_and_trailing_whitespace() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(
            path_from_bytes(b"project-\xff \n")
                .unwrap()
                .as_os_str()
                .as_bytes(),
            b"project-\xff \n"
        );
        let temp = tempfile::tempdir().unwrap();
        // macOS filesystems require UTF-8 names; exercise arbitrary bytes in
        // the decoder above and whitespace through a real repository here.
        let root = temp.path().join("project 文稿 \n");
        fs::create_dir(&root).unwrap();
        perform(&root, Operation::Init).unwrap();
        assert_eq!(snapshot(&root).unwrap().root, root.canonicalize().unwrap());
        fs::write(root.join("example.typ"), "Error: example document").unwrap();
        let result = perform(
            &root,
            Operation::Diff("example.typ".into(), DiffKind::WorkingTree),
        )
        .unwrap();
        assert!(!result.failed);
        assert!(
            result
                .diff
                .unwrap()
                .content
                .unwrap()
                .contains("+Error: example document")
        );
    }
    #[test]
    fn status_preserves_spaces_newlines_and_literal_pathspecs() {
        let entries =
            parse_status(b"?? space name.typ\0 M line\nname.typ\0A  :(glob)*.typ\0").unwrap();
        assert_eq!(entries[1].path, Path::new("line\nname.typ"));
        assert_eq!(entries[2].path, Path::new(":(glob)*.typ"));
    }
    #[test]
    fn repository_can_stage_unstage_commit_and_diff_without_discarding_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        perform(root, Operation::Init).unwrap();
        configure_identity(root);
        let path = PathBuf::from(":(glob)*.typ");
        fs::write(root.join(&path), "first\n").unwrap();
        fs::write(root.join("keep.typ"), "other\n").unwrap();
        perform(root, Operation::Stage(path.clone())).unwrap();
        assert_eq!(
            snapshot(root)
                .unwrap()
                .entries
                .iter()
                .filter(|e| e.index == 'A')
                .count(),
            1
        );
        perform(root, Operation::Unstage(path.clone())).unwrap();
        assert_eq!(
            snapshot(root)
                .unwrap()
                .entries
                .iter()
                .filter(|e| e.index == 'A')
                .count(),
            0
        );
        perform(root, Operation::Stage(path.clone())).unwrap();
        perform(root, Operation::Commit("Initial\n\nDetails".into())).unwrap();
        fs::write(root.join(&path), "second\n").unwrap();
        assert!(
            perform(root, Operation::Diff(path.clone(), DiffKind::WorkingTree))
                .unwrap()
                .diff
                .unwrap()
                .content
                .unwrap()
                .contains("+second")
        );
        perform(root, Operation::Stage(path.clone())).unwrap();
        perform(root, Operation::Unstage(path.clone())).unwrap();
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "second\n");
    }

    fn diff_text(root: &Path, path: &str, kind: DiffKind) -> String {
        perform(root, Operation::Diff(path.into(), kind))
            .unwrap()
            .diff
            .unwrap()
            .content
            .unwrap()
    }

    #[test]
    fn comparisons_distinguish_working_staged_empty_and_binary_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        perform(root, Operation::Init).unwrap();
        configure_identity(root);
        fs::write(root.join("main.typ"), "original\n").unwrap();
        perform(root, Operation::StageAll).unwrap();
        perform(root, Operation::Commit("Initial".into())).unwrap();
        fs::write(root.join("main.typ"), "staged\n").unwrap();
        perform(root, Operation::StageAll).unwrap();
        fs::write(root.join("main.typ"), "working\n").unwrap();
        let working = diff_text(root, "main.typ", DiffKind::WorkingTree);
        let staged = diff_text(root, "main.typ", DiffKind::Staged);
        assert!(working.contains("-staged\n+working"));
        assert!(staged.contains("-original\n+staged"));
        assert!(!staged.contains("+working"));
        perform(root, Operation::StageAll).unwrap();
        assert!(diff_text(root, "main.typ", DiffKind::WorkingTree).is_empty());
        perform(root, Operation::UnstageAll).unwrap();
        assert!(diff_text(root, "main.typ", DiffKind::Staged).is_empty());
        fs::write(root.join("new.typ"), "new file\n").unwrap();
        assert!(diff_text(root, "new.typ", DiffKind::WorkingTree).contains("+new file"));
        fs::write(root.join("image.bin"), [0, 1, 2, 3]).unwrap();
        assert!(diff_text(root, "image.bin", DiffKind::WorkingTree).contains("Binary files"));
    }

    #[test]
    fn unstage_all_preserves_working_files_before_and_after_first_commit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        perform(root, Operation::Init).unwrap();
        configure_identity(root);
        fs::create_dir(root.join("chapter")).unwrap();
        fs::write(root.join("chapter/main.typ"), "first\n").unwrap();
        fs::write(root.join("deleted.typ"), "delete later\n").unwrap();
        perform(root, Operation::StageAll).unwrap();
        fs::write(root.join("chapter/main.typ"), "edited after staging\n").unwrap();
        perform(root, Operation::Unstage("chapter/main.typ".into())).unwrap();
        perform(root, Operation::StageAll).unwrap();
        fs::write(root.join("chapter/main.typ"), "edited again\n").unwrap();
        perform(root, Operation::UnstageAll).unwrap();
        assert!(
            snapshot(root)
                .unwrap()
                .entries
                .iter()
                .all(|entry| !entry.staged())
        );
        assert_eq!(
            fs::read_to_string(root.join("chapter/main.typ")).unwrap(),
            "edited again\n"
        );
        perform(root, Operation::StageAll).unwrap();
        perform(root, Operation::Commit("Initial".into())).unwrap();
        fs::remove_file(root.join("deleted.typ")).unwrap();
        fs::write(root.join("chapter/main.typ"), "latest edit\n").unwrap();
        fs::write(root.join("new.typ"), "new\n").unwrap();
        perform(root, Operation::StageAll).unwrap();
        perform(root, Operation::UnstageAll).unwrap();
        assert!(
            snapshot(root)
                .unwrap()
                .entries
                .iter()
                .all(|entry| !entry.staged())
        );
        assert!(!root.join("deleted.typ").exists());
        assert_eq!(
            fs::read_to_string(root.join("chapter/main.typ")).unwrap(),
            "latest edit\n"
        );
        assert_eq!(fs::read_to_string(root.join("new.typ")).unwrap(), "new\n");
        assert!(text_run(root, &["diff", "--cached"]).unwrap().is_empty());
    }

    #[test]
    fn stage_all_excludes_private_artifacts_and_preserves_literal_filenames() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        perform(root, Operation::Init).unwrap();
        for path in [".tiptoptyp/mirror/main.typ", "nested/.tiptoptyp/cache.bin"] {
            fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            fs::write(root.join(path), "private").unwrap();
        }
        for path in [".tiptoptyp-notes", ":(glob)*.typ", "space and\nnewline.typ"] {
            fs::write(root.join(path), "document").unwrap();
        }
        perform(root, Operation::StageAll).unwrap();
        let entries = snapshot(root).unwrap().entries;
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .all(|entry| entry.staged() && !private_artifact(&entry.path))
        );
        assert!(perform(root, Operation::Stage(".tiptoptyp/mirror/main.typ".into())).is_err());
        // Existing staged artifacts remain visible and can be removed from the
        // index by the panel, without deleting a running preview's files.
        text_run(root, &["add", "--", ".tiptoptyp/mirror/main.typ"]).unwrap();
        assert!(
            snapshot(root)
                .unwrap()
                .entries
                .iter()
                .any(|entry| private_artifact(&entry.path) && entry.staged())
        );
        perform(root, Operation::UnstageAll).unwrap();
        assert!(
            !snapshot(root)
                .unwrap()
                .entries
                .iter()
                .any(|entry| private_artifact(&entry.path))
        );
        assert_eq!(
            fs::read_to_string(root.join(".tiptoptyp/mirror/main.typ")).unwrap(),
            "private"
        );
    }

    #[test]
    fn action_columns_stay_aligned_and_inside_narrow_and_wide_windows() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        for width in [240.0, 560.0, 860.0] {
            let mut harness = Harness::builder()
                .with_size(egui::vec2(width, 900.0))
                .build_ui_state(show_panel, GitPanel::snapshot_fixture());
            harness.run();
            assert!(harness.query_by_label("Git").is_none());
            assert!(harness.query_by_label("Refresh").is_none());
            assert!(harness.query_by_label("Commit").is_none());
            assert!(harness.query_by_label_contains("Choose Diff").is_none());
            let summary = harness.get_by_label("4 files · 2 staged").rect();
            let staging = harness.get_by_label("Stage all").rect();
            assert!(
                summary.bottom() <= staging.top(),
                "{summary:?} vs {staging:?}"
            );
            assert!(summary.left() >= 0.0 && summary.right() <= width);
            let mut columns = Vec::new();
            for labels in [["Diff", "Staged diff"], ["Stage", "Unstage"]] {
                let rects = harness
                    .get_all_by_label(labels[0])
                    .chain(harness.get_all_by_label(labels[1]))
                    .map(|node| node.rect())
                    .collect::<Vec<_>>();
                assert_eq!(rects.len(), 4, "{labels:?}");
                for rect in &rects {
                    assert!(
                        (rect.right() - rects[0].right()).abs() < 0.5,
                        "{labels:?}: {rects:?}"
                    );
                    assert!((rect.width() - rects[0].width()).abs() < 0.5);
                    assert!(
                        rect.left() >= 0.0 && rect.right() <= width,
                        "{labels:?}: {rect:?}"
                    );
                    if width == 240.0 {
                        assert!(rect.width() <= 30.0);
                    }
                }
                columns.push(rects[0]);
            }
            assert!(
                columns
                    .windows(2)
                    .all(|pair| pair[0].right() < pair[1].left())
            );
            let edge = columns.last().unwrap().right();
            for label in ["Push", "Unstage all", "Commit staged changes"] {
                assert!(
                    (harness.get_by_label(label).rect().right() - edge).abs() < 0.5,
                    "{label} at {width}"
                );
            }
            let path = harness
                .get_by_label("chapters/experiments/supplementary-results.typ")
                .rect();
            assert!(
                path.right() < columns[0].left(),
                "path overlaps actions at {width}: {path:?}"
            );
        }
    }

    fn finish_ui_job(harness: &mut egui_kittest::Harness<'_, GitPanel>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while harness.state().job.is_running() {
            assert!(Instant::now() < deadline, "Git UI job timed out");
            thread::sleep(Duration::from_millis(10));
            harness.step();
        }
        harness.run();
        assert!(!harness.state().failed, "{}", harness.state().message);
    }

    #[test]
    fn diff_buttons_collect_polled_worker_results_and_unstage_all_works() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        perform(root, Operation::Init).unwrap();
        configure_identity(root);
        fs::write(root.join("main.typ"), "original\n").unwrap();
        perform(root, Operation::StageAll).unwrap();
        perform(root, Operation::Commit("Initial".into())).unwrap();
        fs::write(root.join("main.typ"), "staged\n").unwrap();
        perform(root, Operation::StageAll).unwrap();
        fs::write(root.join("main.typ"), "working\n").unwrap();
        let panel = GitPanel {
            visible: true,
            workspace: root.into(),
            snapshot: snapshot(root).unwrap(),
            ..Default::default()
        };
        // Match the app: poll the model before rendering its body.
        let mut harness = Harness::builder()
            .with_size(egui::vec2(860.0, 900.0))
            .build_ui_state(
                |ui, panel| {
                    panel.poll(ui.ctx(), root);
                    show_panel(ui, panel);
                },
                panel,
            );
        harness.run();
        let unstage_rect = harness.get_by_label("Unstage").rect();
        let staged_diff_rect = harness.get_by_label("Staged diff").rect();
        harness.get_by_label("Staged diff").click();
        harness.step();
        finish_ui_job(&mut harness);
        harness.get_by_label("Staged changes");
        harness.get_by_label_contains("+staged");
        assert!(harness.query_by_label_contains("+working").is_none());
        harness.get_by_label("Unstage all").click();
        harness.step();
        finish_ui_job(&mut harness);
        assert!(
            harness
                .state()
                .snapshot
                .entries
                .iter()
                .all(|entry| !entry.staged())
        );
        assert_eq!(
            fs::read_to_string(root.join("main.typ")).unwrap(),
            "working\n"
        );
        assert_eq!(
            harness.get_by_label("Stage").rect().width(),
            unstage_rect.width()
        );
        assert_eq!(
            harness.get_by_label("Diff").rect().width(),
            staged_diff_rect.width()
        );
        harness.get_by_label("Diff").click();
        harness.step();
        finish_ui_job(&mut harness);
        harness.get_by_label("Unstaged changes");
        harness.get_by_label_contains("+working");
        harness.get_by_label("Stage").click();
        harness.step();
        finish_ui_job(&mut harness);
        assert_eq!(
            harness.get_by_label("Unstage").rect().width(),
            unstage_rect.width()
        );
        harness.get_by_label("Unstage").click();
        harness.step();
        finish_ui_job(&mut harness);
        harness.get_by_label("Stage");
        assert_eq!(harness.state().snapshot.entries.staged, 0);
    }

    #[test]
    fn diff_view_has_explicit_empty_loading_and_error_states() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut panel = GitPanel::snapshot_fixture();
        panel.diff.as_mut().unwrap().content = None;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(860.0, 900.0))
            .build_ui_state(show_panel, panel);
        harness.run_steps(2);
        harness.get_by_label("Loading diff…");
        for kind in [DiffKind::WorkingTree, DiffKind::Staged] {
            let diff = harness.state_mut().diff.as_mut().unwrap();
            diff.selection.kind = kind;
            diff.content = Some(Ok(String::new().into()));
            harness.run();
            harness.get_by_label(kind.empty_message());
        }
        harness.state_mut().diff.as_mut().unwrap().content = Some(Err("Comparison failed".into()));
        harness.run();
        harness.get_by_label("Comparison failed");
    }

    #[test]
    fn selecting_the_same_diff_action_toggles_the_view_closed() {
        let mut panel = GitPanel::snapshot_fixture();
        panel.start(
            &egui::Context::default(),
            Operation::Diff("main.typ".into(), DiffKind::WorkingTree),
        );
        assert!(panel.diff.is_none());
    }

    #[test]
    fn completed_diff_scrolls_into_view_in_a_short_window() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut panel = GitPanel::snapshot_fixture();
        let selection = panel.diff.as_ref().unwrap().selection.clone();
        let content = panel
            .diff
            .as_mut()
            .unwrap()
            .content
            .take()
            .unwrap()
            .map(|content| content.text.to_string());
        let (sender, receiver) = std::sync::mpsc::channel();
        let workspace = panel.workspace.clone();
        panel
            .job
            .start_and_repaint(
                "git-test-delayed-diff",
                &egui::Context::default(),
                move || {
                    receiver.recv().unwrap();
                    Ok(ResultData {
                        workspace,
                        snapshot: GitPanel::snapshot_fixture().snapshot,
                        output: "Comparison ready".into(),
                        committed: false,
                        failed: false,
                        diff: Some(DiffResult { selection, content }),
                    })
                },
            )
            .unwrap();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(560.0, 400.0))
            .build_ui_state(
                |ui, panel| {
                    panel.poll(ui.ctx(), Path::new("/Projects/research-paper"));
                    show_panel(ui, panel);
                },
                panel,
            );
        harness.run_steps(2);
        sender.send(()).unwrap();
        finish_ui_job(&mut harness);
        let heading = harness.get_by_label("Unstaged changes").rect();
        assert!(
            heading.top() >= 0.0 && heading.bottom() <= 400.0,
            "{heading:?}"
        );
        let changes = harness.get_by_label_contains("+The revised model").rect();
        assert!(
            changes.top() >= heading.bottom() && changes.top() < 400.0,
            "{changes:?}"
        );
    }
}
