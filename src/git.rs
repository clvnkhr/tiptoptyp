//! Basic Git operations. Commands run off the UI thread, with literal paths
//! and no terminal prompts; every result carries its originating workspace.
pub(crate) mod editor;
use crate::{
    theme,
    worker::{ExclusiveJob, LatestJobPoll},
};
use eframe::egui;
use std::{
    env,
    ffi::OsString,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    index: char,
    worktree: char,
}

impl Entry {
    fn staged(&self) -> bool {
        self.index != ' ' && self.index != '?'
    }
    fn unstaged(&self) -> bool {
        self.worktree != ' '
    }
    fn stageable(&self) -> bool {
        self.unstaged() && !private_artifact(&self.path)
    }
}

fn private_artifact(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str() == ".tiptoptyp")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    WorkingTree,
    Staged,
}

impl DiffKind {
    fn title(self) -> &'static str {
        match self {
            Self::WorkingTree => "Unstaged changes",
            Self::Staged => "Staged changes",
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffSelection {
    path: PathBuf,
    kind: DiffKind,
}

#[derive(Debug)]
struct DiffResult {
    selection: DiffSelection,
    content: Result<String, String>,
}

#[derive(Debug)]
struct DiffView {
    selection: DiffSelection,
    // None means the worker is still loading the comparison.
    content: Option<Result<String, String>>,
    reveal: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Snapshot {
    root: PathBuf,
    branch: String,
    entries: Vec<Entry>,
    history: String,
    initialized: bool,
}

#[derive(Debug, Clone)]
enum Operation {
    Refresh,
    Init,
    Stage(PathBuf),
    Unstage(PathBuf),
    StageAll,
    UnstageAll,
    Commit(String),
    Diff(PathBuf, DiffKind),
    Fetch,
    Pull,
    Push,
}

#[derive(Debug)]
struct ResultData {
    workspace: PathBuf,
    snapshot: Snapshot,
    output: String,
    committed: bool,
    failed: bool,
    diff: Option<DiffResult>,
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
            let result = perform(&workspace, operation);
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
                    view.content = Some(result.content);
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

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, workspace: &Path, dirty: bool) {
        // Worker completions repaint their originating viewport. Collect them
        // here too: the main editor need not redraw while this window is active.
        self.poll(ui.ctx(), workspace);
        let mut action = None;
        // Maintenance scans stay out of the visible loading state. They must
        // not dim controls or replace the stable status row on every timer
        // tick; explicit operations still use the normal busy state.
        let busy =
            (self.job.is_running() && !self.background_refresh) || self.pending_operation.is_some();
        let palette = theme::palette(ui.ctx());
        egui::ScrollArea::vertical().id_salt("git-page").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                right_action_row(ui, |ui| {
                    if self.snapshot.initialized {
                        for (label, hint, operation) in [
                            ("Push", "Send local commits to the configured remote.", Operation::Push),
                            ("Pull", "Fetch and fast-forward the current branch. Divergent branches are left unchanged.", Operation::Pull),
                            ("Fetch", "Download remote updates without changing working files.", Operation::Fetch),
                        ] {
                            if ui.add_enabled(!dirty, egui::Button::new(label)).on_hover_text(hint).clicked() {
                                action = Some(operation);
                            }
                        }
                    } else if ui.button("Initialize repository").clicked() {
                        action = Some(Operation::Init);
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(&self.snapshot.branch).strong()).truncate());
                    });
                });
            });
            if dirty {
                ui.colored_label(palette.warning, "Save editor changes before staging or committing. Git uses files on disk.");
            }
            ui.add_space(theme::SPACE.content);
            ui.separator();
            if self.snapshot.initialized {
                let staged = self.snapshot.entries.iter().filter(|entry| entry.staged()).count();
                ui.add_enabled_ui(!busy, |ui| {
                    right_action_row(ui, |ui| {
                        if ui.add_enabled(staged > 0, egui::Button::new("Unstage all"))
                            .on_hover_text("Remove all changes from the staging area. Keep all working files and edits.").clicked() {
                            action = Some(Operation::UnstageAll);
                        }
                        if ui.add_enabled(!dirty && self.snapshot.entries.iter().any(Entry::stageable), egui::Button::new("Stage all"))
                            .on_hover_text("Stage all working changes, excluding .tiptoptyp temporary files.").clicked() {
                            action = Some(Operation::StageAll);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.strong("Changes");
                            ui.weak(format!("{} files · {staged} staged", self.snapshot.entries.len()));
                        });
                    });
                    ui.add_space(theme::SPACE.small);
                    egui::ScrollArea::vertical().id_salt("git-changes").max_height(192.0).auto_shrink([false, true]).show(ui, |ui| {
                        for (row, entry) in self.snapshot.entries.iter().enumerate() {
                            let selected = self.diff.as_ref().is_some_and(|diff| diff.selection.path == entry.path);
                            let fill = if selected { palette.active_row } else if row % 2 == 0 { ui.visuals().faint_bg_color } else { egui::Color32::TRANSPARENT };
                            egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(0, 4)).show(ui, |ui| {
                                ui.push_id(&entry.path, |ui| {
                                    right_action_row(ui, |ui| {
                                        for (label, width, enabled, hint, operation) in [
                                            ("Unstage", 66.0, entry.staged(), "Remove this file's changes from the staging area; keep the working file.", Operation::Unstage(entry.path.clone())),
                                            ("Stage", 52.0, !dirty && entry.stageable(), "Include this file's working changes in the next commit.", Operation::Stage(entry.path.clone())),
                                            ("Staged diff", 88.0, entry.staged(), "Show what is staged for the next commit.", Operation::Diff(entry.path.clone(), DiffKind::Staged)),
                                            ("Diff", 44.0, entry.unstaged(), "Show working changes that have not been staged.", Operation::Diff(entry.path.clone(), DiffKind::WorkingTree)),
                                        ] {
                                            if ui.add_enabled(enabled, egui::Button::new(label).min_size(egui::vec2(width, 24.0))).on_hover_text(hint).clicked() { action = Some(operation); }
                                        }
                                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                            ui.add_space(theme::SPACE.small);
                                            ui.add(egui::Label::new(egui::RichText::new(format!("{}{}", entry.index, entry.worktree)).monospace().color(if entry.staged() { palette.success } else { palette.warning })))
                                                .on_hover_text("Git status: staging area / working file. A added, M modified, D deleted, ? untracked.");
                                            ui.add(egui::Label::new(entry.path.to_string_lossy()).truncate()).on_hover_text(entry.path.display().to_string());
                                        });
                                    });
                                });
                            });
                        }
                        if self.snapshot.entries.is_empty() {
                            ui.add_space(theme::SPACE.content);
                            ui.colored_label(palette.success, "Working tree clean");
                            ui.add_space(theme::SPACE.content);
                        }
                    });
                });
                if self.snapshot.entries.iter().any(|entry| entry.staged() && private_artifact(&entry.path)) {
                    ui.colored_label(palette.warning, "Temporary .tiptoptyp files are already staged. Unstage all keeps these files out of the next commit.");
                }
                ui.add_space(theme::SPACE.content);
                self.show_diff(ui);
                ui.add_space(theme::SPACE.content);
                ui.separator();
                ui.strong("Commit");
                ui.add_enabled(!busy, egui::TextEdit::multiline(&mut self.commit_message).hint_text("Describe your changes…").desired_width(f32::INFINITY).desired_rows(2));
                right_action_row(ui, |ui| {
                    if ui.add_enabled(!busy && !dirty && staged > 0 && !self.commit_message.trim().is_empty(), egui::Button::new("Commit staged changes")).clicked() {
                        action = Some(Operation::Commit(self.commit_message.clone()));
                    }
                });
                egui::CollapsingHeader::new("Recent commits").show(ui, |ui| { ui.monospace(&self.snapshot.history); });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if busy { ui.spinner(); }
                ui.add(egui::Label::new(egui::RichText::new(&self.message).color(if self.failed { palette.error } else { palette.neutral })).selectable(true));
            });
        });
        if let Some(action) = action {
            self.start(ui.ctx(), action);
            ui.ctx().request_repaint();
        }
    }

    fn show_diff(&mut self, ui: &mut egui::Ui) {
        let Some(diff) = &mut self.diff else {
            ui.weak(
                "Choose Diff for unstaged edits, or Staged diff for changes in the next commit.",
            );
            return;
        };
        let palette = theme::palette(ui.ctx());
        let response = egui::Frame::group(ui.style())
            .inner_margin(theme::SPACE.content)
            .show(ui, |ui| {
                right_action_row(ui, |ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.strong(diff.selection.kind.title());
                    });
                });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(diff.selection.path.to_string_lossy()).monospace(),
                    )
                    .truncate(),
                )
                .on_hover_text(diff.selection.path.display().to_string());
                ui.weak(diff.selection.kind.description());
                ui.separator();
                match &diff.content {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading diff…");
                        });
                    }
                    Some(Err(error)) => {
                        ui.colored_label(palette.error, error);
                    }
                    Some(Ok(content)) if content.is_empty() => {
                        ui.label(diff.selection.kind.empty_message());
                    }
                    Some(Ok(content)) => {
                        egui::ScrollArea::both()
                            .id_salt((
                                "git-diff",
                                &diff.selection.path,
                                diff.selection.kind == DiffKind::Staged,
                            ))
                            .max_height(230.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                show_colored_diff(ui, content);
                            });
                    }
                }
            })
            .response;
        if diff.reveal {
            response.scroll_to_me(Some(egui::Align::Center));
            diff.reveal = false;
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
                ],
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
                .into())),
                reveal: false,
            }),
            ..Default::default()
        }
    }
}

/// Reserve action columns from the right before allowing left-hand text to
/// consume the remaining width. Long paths can never displace the buttons.
fn right_action_row(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), contents);
    });
}

fn show_colored_diff(ui: &mut egui::Ui, content: &str) {
    let palette = theme::palette(ui.ctx());
    let mut layout = egui::text::LayoutJob::default();
    for line in content.split_inclusive('\n') {
        let color = if line.starts_with('+') {
            palette.success
        } else if line.starts_with('-') {
            palette.error
        } else if line.starts_with("@@") {
            palette.info
        } else {
            ui.visuals().text_color()
        };
        layout.append(
            line,
            0.0,
            egui::TextFormat {
                font_id: egui::TextStyle::Monospace.resolve(ui.style()),
                color,
                ..Default::default()
            },
        );
    }
    ui.add(egui::Label::new(layout).selectable(true).extend());
}

fn run(root: &Path, args: &[OsString]) -> Result<Vec<u8>, String> {
    run_command(root, args, false)
}

fn git_executable() -> Option<PathBuf> {
    static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
    if let Some(program) = EXECUTABLE.get() {
        return Some(program.clone());
    }
    let program = discover_git_executable()?;
    let _ = EXECUTABLE.set(program.clone());
    Some(program)
}

fn discover_git_executable() -> Option<PathBuf> {
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            #[cfg(windows)]
            {
                let extensions =
                    env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
                for extension in extensions.to_string_lossy().split(';') {
                    let candidate = directory.join(format!("git{extension}"));
                    if is_executable(&candidate) {
                        return Some(candidate);
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let candidate = directory.join("git");
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    // GUI applications on macOS do not always inherit the shell's PATH.
    // Check the system and common Homebrew locations after PATH so a Git
    // installation remains usable when the app was launched from Finder.
    #[cfg(target_os = "macos")]
    for candidate in [
        "/usr/bin/git",
        "/opt/homebrew/bin/git",
        "/usr/local/bin/git",
    ] {
        let candidate = PathBuf::from(candidate);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn run_command(root: &Path, args: &[OsString], diff_exit_status: bool) -> Result<Vec<u8>, String> {
    // File-backed output prevents pipe deadlocks and bounds resident output.
    if !root.is_dir() {
        return Err(format!("Git workspace is unavailable: {}", root.display()));
    }
    let program = git_executable()
        .ok_or_else(|| "Could not start Git: no Git executable was found".to_owned())?;
    let mut stdout = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut stderr = tempfile::tempfile().map_err(|e| e.to_string())?;
    let child = Command::new(program)
        .arg("--no-pager")
        .arg("--literal-pathspecs")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(stdout.try_clone().map_err(|e| e.to_string())?)
        .stderr(stderr.try_clone().map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound && !root.is_dir() {
                format!("Git workspace is unavailable: {}", root.display())
            } else {
                format!("Could not start Git: {error}")
            }
        })?;
    let status = crate::process::wait(child, Duration::from_secs(60), || {
        if stdout.metadata()?.len() > 4 * 1024 * 1024 || stderr.metadata()?.len() > 4 * 1024 * 1024
        {
            return Err(std::io::Error::other(
                "Git output exceeds 4 MiB; narrow the operation in a terminal",
            ));
        }
        Ok(())
    })
    .map_err(|error| format!("Git: {error}"))?;
    let read = |file: &mut fs::File| -> Result<Vec<u8>, String> {
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("Git output exceeds 4 MiB; narrow the operation in a terminal".into());
        }
        Ok(bytes)
    };
    let output = read(&mut stdout)?;
    let errors = read(&mut stderr)?;
    if status.success() || (diff_exit_status && status.code() == Some(1)) {
        Ok(output)
    } else {
        let error = String::from_utf8_lossy(&errors).trim().to_owned();
        Err(if error.is_empty() {
            format!("Git exited with {status}")
        } else {
            error
        })
    }
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

/// Both diff consumers parse unified output, so user presentation settings
/// must not change its line prefixes or remove blank context markers.
fn diff_args() -> Vec<OsString> {
    args(&[
        "-c",
        "diff.suppressBlankEmpty=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--output-indicator-new=+",
        "--output-indicator-old=-",
        "--output-indicator-context= ",
    ])
}
fn text_run(root: &Path, values: &[&str]) -> Result<String, String> {
    run(root, &args(values)).map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
}

fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(
            std::str::from_utf8(bytes).map_err(|e| e.to_string())?,
        ))
    }
}

fn parse_status(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .filter(|record| !record.starts_with(b"## "))
        .map(|record| {
            if record.len() < 4 || record[2] != b' ' {
                return Err("Invalid Git status record".into());
            }
            let path = path_from_bytes(&record[3..])?;
            Ok(Entry {
                path,
                index: record[0] as char,
                worktree: record[1] as char,
            })
        })
        .collect()
}

fn snapshot(workspace: &Path) -> Result<Snapshot, String> {
    let mut snapshot = status_snapshot(workspace)?;
    if snapshot.initialized {
        snapshot.history = text_run(&snapshot.root, &["log", "-10", "--oneline"])
            .unwrap_or_else(|_| "No commits yet".into());
    }
    Ok(snapshot)
}

/// Background decorations need status only, never commit history or a second
/// full status scan just to obtain the branch header.
fn status_snapshot(workspace: &Path) -> Result<Snapshot, String> {
    // No-renames gives one NUL-delimited record per path, including both sides
    // of a rename, without ambiguous quoting or arrow parsing.
    let root_bytes = match run(workspace, &args(&["rev-parse", "--show-toplevel"])) {
        Ok(root) => root,
        Err(error) if error.contains("not a git repository") => {
            return Ok(Snapshot {
                root: workspace.to_path_buf(),
                ..Default::default()
            });
        }
        Err(error) => return Err(error),
    };
    let root = path_from_bytes(root_bytes.strip_suffix(b"\n").unwrap_or(&root_bytes))?;
    let status = run(
        &root,
        &args(&[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "--branch",
            "-z",
            "--no-renames",
            "--untracked-files=all",
        ]),
    )?;
    let mut entries = parse_status(&status)?;
    // A preview mirror is app-owned, even in repositories without a matching
    // ignore rule. Keep tracked/staged artifacts visible so they can be unstaged.
    entries.retain(|entry| entry.index != '?' || !private_artifact(&entry.path));
    let branch =
        String::from_utf8_lossy(status.split(|byte| *byte == 0).next().unwrap_or_default())
            .trim_start_matches("## ")
            .to_owned();
    Ok(Snapshot {
        root,
        branch,
        entries,
        history: String::new(),
        initialized: true,
    })
}

fn perform(workspace: &Path, operation: Operation) -> Result<ResultData, String> {
    if matches!(operation, Operation::Refresh | Operation::Diff(..)) {
        return perform_locked(workspace, operation);
    }
    let root = snapshot(workspace)?.root;
    crate::resource_lock::with_resource(&root, || perform_locked(workspace, operation))
}

fn perform_locked(workspace: &Path, operation: Operation) -> Result<ResultData, String> {
    let before = snapshot(workspace)?;
    let root = &before.root;
    let committed = matches!(operation, Operation::Commit(_));
    // Keep the NUL-delimited pathspec alive until Git has consumed it.
    let mut pathspec = None;
    let command = match operation {
        Operation::Refresh => Vec::new(),
        Operation::Init => args(&["init"]),
        Operation::Stage(path) => {
            if private_artifact(&path) {
                return Err("Temporary .tiptoptyp files are excluded from staging".into());
            }
            let mut command = args(&["add", "--"]);
            command.push(path.into_os_string());
            command
        }
        Operation::Unstage(path) => unstage_command(root, path, false),
        Operation::UnstageAll => unstage_command(root, PathBuf::from("."), true),
        Operation::StageAll => {
            let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
            let mut count = 0;
            for entry in before.entries.iter().filter(|entry| entry.stageable()) {
                file.write_all(entry.path.as_os_str().as_encoded_bytes())
                    .map_err(|e| e.to_string())?;
                file.write_all(&[0]).map_err(|e| e.to_string())?;
                count += 1;
            }
            if count == 0 {
                return Err("No working changes to stage".into());
            }
            file.flush().map_err(|e| e.to_string())?;
            let mut command = args(&[
                "add",
                "--all",
                "--pathspec-file-nul",
                "--pathspec-from-file",
            ]);
            command.push(file.path().as_os_str().to_owned());
            pathspec = Some(file);
            command
        }
        Operation::Commit(message) => {
            if message.trim().is_empty() {
                return Err("Enter a commit message".into());
            }
            let mut command = args(&["commit", "-m"]);
            command.push(message.into());
            command
        }
        Operation::Diff(path, kind) => {
            let untracked = kind == DiffKind::WorkingTree
                && before
                    .entries
                    .iter()
                    .any(|entry| entry.path == path && entry.index == '?');
            let mut command = diff_args();
            if untracked {
                command.push("--no-index".into());
            }
            if kind == DiffKind::Staged {
                command.push("--cached".into());
            }
            command.push("--".into());
            if untracked {
                command.push(if cfg!(windows) { "NUL" } else { "/dev/null" }.into());
            }
            command.push(path.as_os_str().to_owned());
            // --no-index uses exit code 1 for a successful comparison with changes.
            let bytes = run_command(root, &command, untracked)?;
            return Ok(ResultData {
                workspace: workspace.to_path_buf(),
                snapshot: before,
                output: format!("{}: {}", kind.title(), path.display()),
                committed: false,
                failed: false,
                diff: Some(DiffResult {
                    selection: DiffSelection { path, kind },
                    content: Ok(String::from_utf8_lossy(&bytes).into_owned()),
                }),
            });
        }
        Operation::Fetch => args(&["fetch"]),
        Operation::Pull => args(&["pull", "--ff-only"]),
        Operation::Push => args(&["push"]),
    };
    let output = if command.is_empty() {
        "Status refreshed".into()
    } else {
        let bytes = run(root, &command)?;
        if bytes.is_empty() {
            "Git operation completed".into()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };
    drop(pathspec);
    Ok(ResultData {
        workspace: workspace.to_path_buf(),
        snapshot: if command.is_empty() {
            before
        } else {
            snapshot(workspace)?
        },
        output,
        committed,
        failed: false,
        diff: None,
    })
}

fn unstage_command(root: &Path, path: PathBuf, all: bool) -> Vec<OsString> {
    let unborn = text_run(root, &["rev-parse", "--verify", "HEAD"]).is_err();
    let mut command = if unborn {
        // There is no HEAD to restore yet. --cached changes only the index;
        // force also permits files edited again after their initial staging.
        let mut command = args(&["rm", "--cached", "--force"]);
        if all {
            command.push("-r".into());
        }
        command
    } else {
        args(&["restore", "--staged"])
    };
    command.push("--".into());
    command.push(path.into_os_string());
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Instant};

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
            .build_ui_state(|ui, panel| panel.show(ui, root, false), panel);
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
            .build_ui_state(|ui, panel| panel.show(ui, root, false), panel);
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
        for width in [560.0, 860.0] {
            let mut harness = Harness::builder()
                .with_size(egui::vec2(width, 900.0))
                .build_ui_state(
                    |ui, panel| panel.show(ui, Path::new("/Projects/research-paper"), false),
                    GitPanel::snapshot_fixture(),
                );
            harness.run();
            assert!(harness.query_by_label("Git").is_none());
            assert!(harness.query_by_label("Refresh").is_none());
            let mut columns = Vec::new();
            for label in ["Diff", "Staged diff", "Stage", "Unstage"] {
                let rects = harness
                    .get_all_by_label(label)
                    .map(|node| node.rect())
                    .collect::<Vec<_>>();
                assert_eq!(rects.len(), 4, "{label}");
                for rect in &rects {
                    assert!(
                        (rect.right() - rects[0].right()).abs() < 0.5,
                        "{label}: {rects:?}"
                    );
                    assert!(
                        rect.left() >= 0.0 && rect.right() <= width,
                        "{label}: {rect:?}"
                    );
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
    fn diff_buttons_collect_worker_results_in_the_git_window_and_unstage_all_works() {
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
        // This harness only renders the child panel; no root-window poll runs.
        let mut harness = Harness::builder()
            .with_size(egui::vec2(860.0, 900.0))
            .build_ui_state(|ui, panel| panel.show(ui, root, false), panel);
        harness.run();
        harness.get_by_label("Diff").click();
        harness.step();
        finish_ui_job(&mut harness);
        harness.get_by_label("Unstaged changes");
        harness.get_by_label_contains("+working");
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
    }

    #[test]
    fn diff_view_has_explicit_empty_loading_and_error_states() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut panel = GitPanel::snapshot_fixture();
        panel.diff.as_mut().unwrap().content = None;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(860.0, 900.0))
            .build_ui_state(
                |ui, panel| panel.show(ui, Path::new("/Projects/research-paper"), false),
                panel,
            );
        harness.run_steps(2);
        harness.get_by_label("Loading diff…");
        for kind in [DiffKind::WorkingTree, DiffKind::Staged] {
            let diff = harness.state_mut().diff.as_mut().unwrap();
            diff.selection.kind = kind;
            diff.content = Some(Ok(String::new()));
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
        let content = panel.diff.as_mut().unwrap().content.take().unwrap();
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
                |ui, panel| panel.show(ui, Path::new("/Projects/research-paper"), false),
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
