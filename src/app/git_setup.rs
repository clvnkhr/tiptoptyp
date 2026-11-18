//! Explicit repository-local setup. Git IO stays on one worker; the form is pure UI.
use super::*;
use crate::git::repository::setup::{self, SetupRequest, SetupSnapshot};
use crate::github as service;
use crate::worker::{ExclusiveJob, OperationSummary};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

mod github;

pub(super) const SALT: &str = "tiptoptyp-git-setup";

#[derive(Default)]
pub(super) struct State {
    dialog: Option<Dialog>,
    operation: SetupOperation,
    next: u64,
    error: Option<String>,
    archive: Option<Archive>,
    default_name: String,
}

struct Dialog {
    id: u64,
    workspace: PathBuf,
    snapshot: Option<SetupSnapshot>,
    draft: Draft,
    error: Option<String>,
    edit_events: Vec<egui::Event>,
    github: github::Form,
}

#[derive(Default)]
struct Draft {
    name: String,
    email: String,
    origin: String,
    branch: String,
    focus: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Inspect,
    Apply,
    SignIn,
    CreateGitHub,
    AttachGitHub,
    ForgetGitHub,
}

impl Phase {
    fn mutates_repository(self) -> bool {
        matches!(
            self,
            Self::Apply | Self::CreateGitHub | Self::AttachGitHub | Self::ForgetGitHub
        )
    }
}

struct Archive {
    workspace: PathBuf,
    local: SetupSnapshot,
    created: service::CreatedRepository,
}

enum Outcome {
    Inspected {
        snapshot: SetupSnapshot,
        created: Option<service::CreatedRepository>,
    },
    Snapshot(SetupSnapshot),
    AttachedGitHub(service::AttachedRepository),
    Account(service::Account),
    Created {
        local: SetupSnapshot,
        created: service::CreatedRepository,
    },
    CreateFailed {
        local: SetupSnapshot,
        message: String,
    },
}

struct Pending {
    id: u64,
    workspace: PathBuf,
    phase: Phase,
}

#[derive(Default)]
enum SetupOperation {
    #[default]
    Idle,
    Running {
        pending: Pending,
        job: ExclusiveJob<Reply>,
        cancel: Cancellation,
        progress: Option<mpsc::Receiver<(u64, service::Progress)>>,
    },
}

impl SetupOperation {
    fn is_running(&self) -> bool {
        matches!(self, Self::Running { job, .. } if job.is_running())
    }

    fn pending(&self) -> Option<&Pending> {
        match self {
            Self::Idle => None,
            Self::Running { pending, .. } => Some(pending),
        }
    }

    fn cancel_requested(&self) -> bool {
        matches!(self, Self::Running { cancel, .. } if cancel.load(Ordering::Acquire))
    }

    fn cancel(&mut self) {
        if let Self::Running {
            cancel, progress, ..
        } = self
        {
            cancel.store(true, Ordering::Release);
            *progress = None;
        }
    }

    fn poll(&mut self) -> LatestJobPoll<Reply> {
        let Self::Running { pending, job, .. } = self else {
            return LatestJobPoll::Idle;
        };
        match job.poll() {
            LatestJobPoll::Ready(reply)
                if pending.id != reply.id
                    || pending.workspace != reply.workspace
                    || pending.phase != reply.phase =>
            {
                LatestJobPoll::Failed("Git setup returned a result for another request".into())
            }
            result => result,
        }
    }

    #[cfg(test)]
    fn test_pending(pending: Pending) -> Self {
        // Pure admission tests inject the reply directly. Tests that exercise
        // worker polling install their real worker through job_for_test().
        Self::Running {
            pending,
            job: ExclusiveJob::default(),
            cancel: Cancellation::default(),
            progress: None,
        }
    }

    #[cfg(test)]
    fn job_for_test(&mut self) -> &mut ExclusiveJob<Reply> {
        let Self::Running { job, .. } = self else {
            panic!("a request must own the test worker")
        };
        job
    }

    #[cfg(test)]
    fn cancellation_for_test(&self) -> &Arc<AtomicBool> {
        let Self::Running { cancel, .. } = self else {
            panic!("a request must own cancellation")
        };
        &cancel.0
    }

    #[cfg(test)]
    fn set_progress_for_test(&mut self, receiver: mpsc::Receiver<(u64, service::Progress)>) {
        let Self::Running { progress, .. } = self else {
            panic!("a request must own progress")
        };
        *progress = Some(receiver);
    }
}

struct Reply {
    id: u64,
    workspace: PathBuf,
    phase: Phase,
    result: Result<Outcome, String>,
}

impl OperationSummary for Reply {
    fn completion_summary(&self) -> String {
        if let Ok(Outcome::Created { created, .. }) = &self.result {
            return format!(
                "{} {}{}",
                if created.confirmed() {
                    "Created GitHub repository"
                } else {
                    "Review GitHub creation at"
                },
                created.html_url,
                created
                    .snapshot
                    .as_ref()
                    .err()
                    .map_or(String::new(), |error| format!(
                        "; local connection failed: {error}"
                    ))
            );
        }
        if let Ok(Outcome::CreateFailed { message, .. }) | Err(message) = &self.result {
            return format!("Git setup in {}: {message}", self.workspace.display());
        }
        let operation = if self.phase == Phase::Apply {
            "Git setup"
        } else {
            "Git settings"
        };
        format!(
            "{operation} {} in {}",
            if self.result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            self.workspace.display(),
        )
    }
}

#[derive(Default)]
struct Cancellation(Arc<AtomicBool>);

impl std::ops::Deref for Cancellation {
    type Target = Arc<AtomicBool>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct Completion {
    workspace: PathBuf,
    phase: Phase,
    message: String,
    failed: bool,
    close_dialog: bool,
}

impl State {
    pub(super) fn queue_edit_event(&mut self, event: egui::Event) {
        if let Some(dialog) = &mut self.dialog {
            dialog.edit_events.push(event);
        }
    }
    pub(super) fn visible(&self) -> bool {
        self.dialog.is_some()
    }
    pub(super) fn applying(&self) -> bool {
        self.operation.is_running()
            && self
                .operation
                .pending()
                .is_some_and(|pending| pending.phase.mutates_repository())
    }
    pub(super) fn activity(&self) -> crate::activity::Activity {
        crate::activity::Activity::work(self.operation.is_running(), false, self.error.as_deref())
    }
    fn spawn(
        &mut self,
        phase: Phase,
        label: &'static str,
        context: &egui::Context,
        progress: Option<mpsc::Receiver<(u64, service::Progress)>>,
        work: impl FnOnce(PathBuf, Arc<AtomicBool>, u64) -> Result<Outcome, String> + Send + 'static,
    ) {
        if self.operation.is_running() {
            return;
        }
        let Some(id) = self.next.checked_add(1) else {
            self.error = Some("Git setup cannot start another request. Restart tiptoptyp.".into());
            if let Some(dialog) = &mut self.dialog {
                dialog.error.clone_from(&self.error);
            }
            return;
        };
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        self.next = id;
        dialog.id = id;
        dialog.error = None;
        self.error = None;
        debug_assert!(progress.is_none() || phase == Phase::SignIn);
        let cancellation = Cancellation::default();
        let cancel = cancellation.0.clone();
        let workspace = dialog.workspace.clone();
        let pending = Pending {
            id,
            workspace: workspace.clone(),
            phase,
        };
        let mut job = ExclusiveJob::default();
        if let Err(error) = job.start_and_repaint(label, context, move || {
            let result = work(workspace.clone(), cancel, id);
            Ok(Reply {
                id,
                workspace,
                phase,
                result,
            })
        }) {
            self.operation = SetupOperation::Idle;
            self.error = Some(error.clone());
            self.dialog.as_mut().unwrap().error = Some(error);
        } else {
            self.operation = SetupOperation::Running {
                pending,
                job,
                cancel: cancellation,
                progress,
            };
        }
    }
    fn start(&mut self, workspace: PathBuf, context: &egui::Context) {
        if self.operation.is_running() {
            return;
        }
        if self.next == u64::MAX {
            self.error = Some("Git setup cannot start another request. Restart tiptoptyp.".into());
            return;
        }
        self.dialog = Some(Dialog {
            id: 0,
            workspace,
            snapshot: None,
            draft: Draft::default(),
            error: None,
            edit_events: Vec::new(),
            github: github::Form {
                name: self.default_name.clone(),
                ..Default::default()
            },
        });
        self.spawn(
            Phase::Inspect,
            "Read Git settings",
            context,
            None,
            |workspace, _, _| {
                let snapshot = setup::inspect(&workspace)?;
                let created = service::load_created(&workspace)?;
                Ok(Outcome::Inspected { snapshot, created })
            },
        );
    }
    fn poll_progress(&mut self, current: &Path) {
        let SetupOperation::Running {
            pending,
            progress: Some(receiver),
            ..
        } = &self.operation
        else {
            return;
        };
        for _ in 0..8 {
            let Ok((id, progress)) = receiver.try_recv() else {
                break;
            };
            if pending.id == id
                && pending.phase == Phase::SignIn
                && pending.workspace == current
                && let Some(dialog) = &mut self.dialog
                && dialog.id == id
                && dialog.workspace == current
            {
                dialog.github.progress(progress);
            }
        }
    }
    fn accept(&mut self, reply: Reply, current: &Path) -> Option<Completion> {
        let pending = self.operation.pending()?;
        if pending.id != reply.id
            || pending.workspace != reply.workspace
            || pending.phase != reply.phase
        {
            return None;
        }
        let cancelled = self.operation.cancel_requested();
        self.operation = SetupOperation::Idle;
        let owns = self.dialog.as_ref().is_some_and(|dialog| {
            dialog.id == reply.id
                && dialog.workspace == reply.workspace
                && current == reply.workspace
        });
        if owns {
            let dialog = self.dialog.as_mut().unwrap();
            dialog.github.finish_sign_in();
        }
        let mut close_dialog = false;
        let (message, failed) = match reply.result {
            Ok(Outcome::Inspected { snapshot, created }) => {
                self.error = None;
                if owns {
                    let recovered = created.or_else(|| {
                        self.archive
                            .as_ref()
                            .filter(|archive| {
                                archive.workspace == reply.workspace
                                    && archive.local.root == snapshot.root
                            })
                            .map(|archive| archive.created.clone())
                    });
                    let dialog = self.dialog.as_mut().unwrap();
                    dialog.draft = Draft {
                        name: snapshot.author_name.clone(),
                        email: snapshot.author_email.clone(),
                        origin: snapshot.origin.clone(),
                        branch: if snapshot.initialized {
                            snapshot.branch.clone()
                        } else {
                            "main".into()
                        },
                        focus: true,
                    };
                    if dialog.github.name.is_empty() {
                        dialog.github.name = github::repository_name(
                            snapshot
                                .workspace
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("paper"),
                        );
                    }
                    if let Some(mut created) = recovered {
                        if created.confirmed()
                            && snapshot.initialized
                            && snapshot.origin == created.clone_url
                        {
                            created.snapshot = Ok(snapshot.clone());
                        } else if !snapshot.origin.is_empty()
                            && snapshot.origin != created.clone_url
                        {
                            created.snapshot = Err("This working copy uses another remote. Clear it before retrying this connection.".into());
                        }
                        dialog.github.selected =
                            snapshot.origin.is_empty() && created.snapshot.is_err();
                        dialog.github.name.clone_from(&created.name);
                        dialog.github.private = created.private;
                        dialog.github.created = Some(created.clone());
                        self.archive = Some(Archive {
                            workspace: reply.workspace,
                            local: snapshot.clone(),
                            created,
                        });
                    }
                    dialog.snapshot = Some(snapshot);
                    dialog.error = None;
                }
                return None;
            }
            Ok(Outcome::Snapshot(snapshot)) => {
                self.error = None;
                if reply.phase == Phase::ForgetGitHub {
                    self.archive = None;
                    if owns {
                        let dialog = self.dialog.as_mut().unwrap();
                        dialog.github.created = None;
                        dialog.draft.refresh(&snapshot);
                        dialog.snapshot = Some(snapshot);
                        dialog.error = None;
                    }
                    ("No GitHub repository exists at that address. Review the name before creating one.".into(), false)
                } else {
                    let message = format!("Git configuration saved in {}", snapshot.root.display());
                    if owns {
                        self.dialog = None;
                        close_dialog = true;
                    }
                    (message, false)
                }
            }
            Ok(Outcome::AttachedGitHub(attached)) => {
                let (snapshot, created) = attached.into_parts();
                self.error = None;
                self.archive = Some(Archive {
                    workspace: reply.workspace.clone(),
                    local: snapshot.clone(),
                    created: created.clone(),
                });
                if owns {
                    let dialog = self.dialog.as_mut().unwrap();
                    dialog.github.created = Some(created);
                    dialog.draft.refresh(&snapshot);
                    dialog.snapshot = Some(snapshot);
                    dialog.error = None;
                }
                (
                    "GitHub repository connected. Commit and push when you are ready.".into(),
                    false,
                )
            }
            Ok(Outcome::Account(account)) => {
                self.error = None;
                let message = format!("Signed in to GitHub as {}", account.login);
                if owns {
                    self.dialog.as_mut().unwrap().github.account = Some(account);
                }
                (message, false)
            }
            Ok(Outcome::Created { local, created }) => {
                let failed = created.snapshot.is_err();
                let message = if created.confirmed() {
                    format!(
                        "Created {}{}",
                        created.html_url,
                        created
                            .snapshot
                            .as_ref()
                            .err()
                            .map_or(String::new(), |error| format!(". {error}"))
                    )
                } else {
                    format!(
                        "Check {} before creating another repository.",
                        created.html_url
                    )
                };
                if current == reply.workspace {
                    self.error = failed.then(|| message.clone());
                }
                self.archive = Some(Archive {
                    workspace: reply.workspace.clone(),
                    local: local.clone(),
                    created: created.clone(),
                });
                if owns {
                    let dialog = self.dialog.as_mut().unwrap();
                    let snapshot = created.snapshot.as_ref().cloned().unwrap_or(local);
                    dialog.draft.refresh(&snapshot);
                    dialog.snapshot = Some(snapshot);
                    dialog.github.created = Some(created);
                    dialog.error = None;
                }
                (message, failed)
            }
            Ok(Outcome::CreateFailed { local, message }) => {
                if owns {
                    let dialog = self.dialog.as_mut().unwrap();
                    dialog.draft.refresh(&local);
                    dialog.snapshot = Some(local);
                    dialog.error = Some(message.clone());
                }
                if current == reply.workspace {
                    self.error = Some(message.clone());
                }
                (message, true)
            }
            Err(error) => {
                if reply.phase == Phase::SignIn && cancelled {
                    if current == reply.workspace {
                        self.error = None;
                    }
                    if owns {
                        self.dialog.as_mut().unwrap().error = None;
                    }
                    return Some(Completion {
                        workspace: reply.workspace,
                        phase: reply.phase,
                        message: "GitHub sign-in cancelled.".into(),
                        failed: false,
                        close_dialog: false,
                    });
                }
                if current == reply.workspace {
                    self.error = Some(error.clone());
                }
                if owns {
                    self.dialog.as_mut().unwrap().error = Some(error.clone());
                }
                (error, true)
            }
        };
        Some(Completion {
            workspace: reply.workspace,
            phase: reply.phase,
            message,
            failed,
            close_dialog,
        })
    }
    fn request(&self) -> Result<SetupRequest, String> {
        let dialog = self.dialog.as_ref().ok_or("Reopen Git setup.")?;
        let expected = dialog
            .snapshot
            .as_ref()
            .ok_or("Wait for Git settings to load.")?;
        if !dialog.draft.ready(expected.initialized) {
            return Err("Enter an author name, email and initial branch.".into());
        }
        Ok(SetupRequest {
            expected: expected.clone(),
            author_name: dialog.draft.name.trim().into(),
            author_email: dialog.draft.email.trim().into(),
            origin: dialog.draft.origin.trim().into(),
            initial_branch: dialog.draft.branch.trim().into(),
        })
    }
    fn set_error(&mut self, error: String) {
        self.error = Some(error.clone());
        if let Some(dialog) = &mut self.dialog {
            dialog.error = Some(error);
        }
    }
    fn apply(&mut self, context: &egui::Context) {
        if self.operation.is_running() {
            return;
        }
        let request = match self.request() {
            Ok(request) => request,
            Err(error) => {
                self.set_error(error);
                return;
            }
        };
        self.spawn(
            Phase::Apply,
            "Configure Git",
            context,
            None,
            move |workspace, _, _| setup::apply(&workspace, request).map(Outcome::Snapshot),
        );
    }
    fn sign_in(&mut self, context: &egui::Context) {
        if self.operation.is_running() {
            return;
        }
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let reuse_existing = dialog.github.account.is_none();
        dialog.github.begin_sign_in();
        let (sender, receiver) = mpsc::sync_channel(8);
        let repaint = crate::worker::RepaintTarget::current(context);
        self.spawn(
            Phase::SignIn,
            "Sign in to GitHub",
            context,
            Some(receiver),
            move |_, cancel, id| {
                if reuse_existing && let Ok(Some(account)) = service::inspect_account(&cancel) {
                    return Ok(Outcome::Account(account));
                }
                if cancel.load(Ordering::Acquire) {
                    return Err("GitHub sign-in cancelled.".into());
                }
                service::sign_in(&cancel, |progress| {
                    let _ = sender.try_send((id, progress));
                    repaint.request_repaint();
                })
                .map(Outcome::Account)
            },
        );
        if !self.operation.is_running()
            && let Some(dialog) = &mut self.dialog
        {
            dialog.github.finish_sign_in();
        }
    }
    fn cancel_github(&mut self) {
        self.operation.cancel();
        if let Some(dialog) = &mut self.dialog {
            dialog.github.cancel_sign_in();
        }
    }
    fn create_github(&mut self, context: &egui::Context) {
        if self.operation.is_running() {
            return;
        }
        let mut request = match self.request() {
            Ok(request) => request,
            Err(error) => {
                self.set_error(error);
                return;
            }
        };
        if !request.expected.origin.is_empty() {
            self.set_error("Clear the existing remote and save configuration before creating a GitHub repository.".into());
            return;
        }
        let Some(dialog) = &self.dialog else {
            return;
        };
        if !dialog.github.ready() {
            self.set_error("Sign in and enter a GitHub repository name.".into());
            return;
        }
        let account = dialog.github.account.clone().unwrap();
        let name = dialog.github.name.trim().to_owned();
        let private = dialog.github.private;
        request.origin.clear();
        self.spawn(
            Phase::CreateGitHub,
            "Create GitHub repository",
            context,
            None,
            move |workspace, cancel, _| {
                if cancel.load(Ordering::Acquire) {
                    return Err("GitHub creation cancelled.".into());
                }
                let local = setup::apply(&workspace, request)?;
                let create = service::CreateRequest {
                    expected: local.clone(),
                    account,
                    name,
                    private,
                };
                match service::create_repository(&workspace, create, &cancel) {
                    Ok(created) => Ok(Outcome::Created { local, created }),
                    Err(message) => match service::load_created(&workspace) {
                        Ok(Some(created)) => Ok(Outcome::Created { local, created }),
                        _ => Ok(Outcome::CreateFailed { local, message }),
                    },
                }
            },
        );
    }
    fn attach_github(&mut self, context: &egui::Context, forget: bool) {
        if self.operation.is_running() {
            return;
        }
        let Some(dialog) = &self.dialog else {
            return;
        };
        let Some(created) = dialog.github.created.clone() else {
            return;
        };
        let Some(expected) = &dialog.snapshot else {
            return;
        };
        let root = expected.root.clone();
        self.spawn(
            if forget {
                Phase::ForgetGitHub
            } else {
                Phase::AttachGitHub
            },
            "Review GitHub connection",
            context,
            None,
            move |workspace, cancel, _| {
                let expected = setup::inspect(&workspace)?;
                if expected.root != root {
                    return Err("The repository location changed. Reopen Git setup.".into());
                }
                if forget {
                    service::forget_missing_creation(&workspace, &created, &cancel)?;
                    setup::inspect(&workspace).map(Outcome::Snapshot)
                } else {
                    service::attach_repository(&workspace, expected, &created, &cancel)
                        .map(Outcome::AttachedGitHub)
                }
            },
        );
    }
}

impl Draft {
    fn refresh(&mut self, snapshot: &SetupSnapshot) {
        self.name.clone_from(&snapshot.author_name);
        self.email.clone_from(&snapshot.author_email);
        self.origin.clone_from(&snapshot.origin);
        self.branch.clone_from(&snapshot.branch);
    }

    fn ready(&self, initialized: bool) -> bool {
        !self.name.trim().is_empty()
            && !self.email.trim().is_empty()
            && (initialized || !self.branch.trim().is_empty())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Apply,
    Close,
    Retry,
    SignInGitHub,
    CancelGitHub,
    CreateGitHub,
    AttachGitHub,
    ForgetGitHub,
}

fn field(ui: &mut egui::Ui, title: &str, id: &str, value: &mut String) -> egui::Response {
    let label = ui.label(title);
    let response = ui.add(
        egui::TextEdit::singleline(value)
            .id_salt(id)
            .desired_width(f32::INFINITY),
    );
    let response = response.labelled_by(label.id);
    observe(id, &response);
    response
}

struct FormInput<'a> {
    root: &'a Path,
    initialized: bool,
    busy: bool,
    local_copy_pending: bool,
    existing_origin: bool,
    operation: Option<Phase>,
    error: Option<&'a str>,
}

fn show_form(
    ui: &mut egui::Ui,
    draft: &mut Draft,
    github: &mut github::Form,
    input: FormInput<'_>,
) -> Option<Action> {
    ui.strong(if input.initialized {
        "Configure Git"
    } else {
        "Set up Git"
    });
    ui.label(egui::RichText::new(input.root.display().to_string()).monospace());
    let body_height = (ui.available_height() - 48.0).max(80.0);
    let mut action = None;
    egui::ScrollArea::vertical().id_salt("git-setup-fields").max_height(body_height).show(ui, |ui| {
        ui.add_enabled_ui(!input.busy, |ui| {
            ui.strong("Commit author · this repository");
            let name = field(ui, "Author name", "name", &mut draft.name);
            if draft.focus { name.request_focus(); draft.focus = false; }
            field(ui, "Author email", "email", &mut draft.email);
            if input.initialized {
                ui.horizontal(|ui| { ui.label("Branch"); ui.monospace(&draft.branch); });
            } else { field(ui, "Initial branch", "branch", &mut draft.branch); }
            ui.horizontal(|ui| {
                let existing = ui.radio_value(&mut github.selected, false, "Existing remote / local only");
                observe("remote_existing", &existing);
                let hosted = ui.radio_value(&mut github.selected, true, "GitHub");
                observe("remote_github", &hosted);
            });
            if !github.selected { field(ui, "Remote URL (origin, optional)", "origin", &mut draft.origin); }
        });
        if github.selected {
            if let Some(selected) = github.show(ui, input.busy) { action = Some(selected); }
            if input.existing_origin && github.created.is_none() {
                ui.colored_label(theme::palette(ui.ctx()).warning, "Clear the existing remote and save configuration before creating a GitHub repository.");
            }
        }
        if let Some(error) = input.error { ui.colored_label(error_color(ui.ctx()), error); }
        if input.local_copy_pending {
            ui.colored_label(theme::palette(ui.ctx()).warning, "Wait for Overleaf files to finish saving locally.");
        }
        if input.busy && input.operation != Some(Phase::SignIn) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(match input.operation {
                    Some(Phase::CreateGitHub) => "Creating GitHub repository…",
                    Some(Phase::AttachGitHub | Phase::ForgetGitHub) => "Checking GitHub repository…",
                    _ => "Saving Git configuration…",
                });
            });
        }
    });
    ui.add_space(theme::SPACE.control);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let completed = github.selected && github.created.is_some();
        let ready = completed
            || draft.ready(input.initialized)
                && (!github.selected || github.ready() && !input.existing_origin);
        let label = if completed {
            "Done"
        } else if github.selected {
            "Create on GitHub"
        } else if input.initialized {
            "Save configuration"
        } else {
            "Create repository"
        };
        let submit = ui.add_enabled(
            !input.busy && !input.local_copy_pending && ready,
            egui::Button::new(label),
        );
        observe("submit", &submit);
        if submit.clicked() {
            action = Some(if completed {
                Action::Close
            } else if github.selected {
                Action::CreateGitHub
            } else {
                Action::Apply
            });
        }
        let close = ui.button(if input.busy && input.operation == Some(Phase::SignIn) {
            "Cancel sign-in"
        } else if input.busy {
            "Close"
        } else {
            "Cancel"
        });
        observe("cancel", &close);
        if close.clicked() {
            action = Some(Action::Close);
        }
    });
    action
}

fn observe(id: &str, response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(&format!("git.setup.{id}"), response);
    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
    let _ = (id, response);
}

impl EditorApp {
    pub(super) fn begin_git_setup(&mut self, context: &egui::Context) {
        if self.git_setup.operation.is_running() {
            self.notice = Some(Notice {
                message: "Git setup is still running.".into(),
                kind: NoticeKind::Info,
            });
            context.request_repaint();
            return;
        }
        if self.document_flow_busy()
            || self.git.operation_busy()
            || self.process_close_pending()
            || self.overleaf_local_copy_pending()
        {
            return;
        }
        let name = self
            .overleaf_explorer()
            .map(|(_, name, _)| name.to_owned())
            .or_else(|| {
                self.workspace_root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "paper".into());
        self.git_setup.default_name = github::repository_name(&name);
        self.git_setup.start(self.workspace_root.clone(), context);
        if !self.git_setup.visible()
            && let Some(error) = &self.git_setup.error
        {
            self.notice = Some(Notice {
                message: error.clone(),
                kind: NoticeKind::Error,
            });
        }
        self.close_app_popup();
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        context.request_repaint();
    }

    pub(super) fn close_git_setup(&mut self, context: &egui::Context) {
        if self.git_setup.operation.pending().is_some_and(|pending| {
            matches!(
                pending.phase,
                Phase::Inspect
                    | Phase::SignIn
                    | Phase::CreateGitHub
                    | Phase::AttachGitHub
                    | Phase::ForgetGitHub
            )
        }) {
            self.git_setup.cancel_github();
        }
        if self.git_setup.dialog.take().is_some() {
            ChildViewHost::close(context, SALT);
            if self
                .pending_widget_paste
                .is_some_and(|pending| pending.viewport == scoped_child_viewport_id(context, SALT))
            {
                self.pending_widget_paste = None;
            }
            crate::window_host::return_focus(context);
        }
    }

    pub(super) fn poll_git_setup(&mut self, context: &egui::Context) {
        if self
            .git_setup
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.workspace != self.workspace_root)
        {
            self.close_git_setup(context);
        }
        self.git_setup.poll_progress(&self.workspace_root);
        let reply = match self.git_setup.operation.poll() {
            LatestJobPoll::Idle | LatestJobPoll::Pending => return,
            LatestJobPoll::Ready(reply) => reply,
            LatestJobPoll::Failed(error) => {
                let Some(pending) = self.git_setup.operation.pending() else {
                    return;
                };
                Reply {
                    id: pending.id,
                    workspace: pending.workspace.clone(),
                    phase: pending.phase,
                    result: Err(error),
                }
            }
        };
        if let Some(completion) = self.git_setup.accept(reply, &self.workspace_root) {
            let kind = if completion.failed {
                NoticeKind::Error
            } else {
                NoticeKind::Success
            };
            if completion.workspace == self.workspace_root {
                if completion.phase.mutates_repository() {
                    self.git.request_refresh();
                    self.git_editor.request_refresh();
                    if completion.close_dialog {
                        ChildViewHost::close(context, SALT);
                        crate::window_host::return_focus(context);
                    }
                }
                self.notice = Some(Notice {
                    message: completion.message,
                    kind,
                });
                self.record_notice_transition();
            } else {
                self.push_status_log(
                    format!("{}: {}", completion.workspace.display(), completion.message),
                    kind,
                );
            }
            context.request_repaint();
        }
    }

    pub(super) fn show_git_setup(&mut self, context: &egui::Context) {
        let local_copy_pending = self.overleaf_local_copy_pending();
        let operation = self
            .git_setup
            .operation
            .pending()
            .map(|pending| pending.phase);
        let Some(dialog) = &mut self.git_setup.dialog else {
            return;
        };
        let Some(owner) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let shortcuts = self.settings.effective_shortcuts();
        let child = scoped_child_viewport_id(context, SALT);
        let admitted = self
            .pending_widget_paste
            .is_some_and(|pending| pending.admits(child, context.cumulative_frame_nr()));
        let busy = self.git_setup.operation.is_running();
        let mut action = None;
        let mut paste_requested = false;
        let mut paste_admitted = false;
        ChildViewHost::show(
            context,
            &self.captures,
            ChildViewSpec::modal(SALT, "Git setup", owner, "git-setup"),
            theme,
            &style,
            |ui, input| {
                handle_window_management_shortcuts(ui.ctx(), ui.ctx().viewport_id(), &shortcuts);
                let edits = prepare_child_widget_edit_events(ui.ctx(), &shortcuts, admitted);
                paste_requested |= edits.paste_requested;
                paste_admitted |= edits.requested_paste_admitted;
                let _queued =
                    crate::child_view::begin_queued_edit_events(ui.ctx(), &mut dialog.edit_events);
                if input.close_requested || input.escape_pressed {
                    action = Some(Action::Close);
                }
                egui::Area::new(viewport_scoped_id(ui.ctx(), "git-setup-card"))
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            ui.set_width((owner.width() - 48.0).clamp(1.0, 560.0));
                            ui.set_max_height((owner.height() - 48.0).max(80.0));
                            if let Some(snapshot) = &dialog.snapshot {
                                if let Some(clicked) = show_form(
                                    ui,
                                    &mut dialog.draft,
                                    &mut dialog.github,
                                    FormInput {
                                        root: &snapshot.root,
                                        initialized: snapshot.initialized,
                                        busy,
                                        local_copy_pending,
                                        existing_origin: !snapshot.origin.is_empty(),
                                        operation,
                                        error: dialog.error.as_deref(),
                                    },
                                ) && action != Some(Action::Close)
                                {
                                    action = Some(clicked);
                                }
                            } else {
                                ui.strong("Set up Git");
                                if busy {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label("Loading Git settings…");
                                    });
                                }
                                if let Some(error) = &dialog.error {
                                    ui.colored_label(error_color(ui.ctx()), error);
                                    if error.contains("no Git executable") {
                                        ui.hyperlink_to("Get Git", "https://git-scm.com/downloads");
                                    }
                                }
                                ui.horizontal(|ui| {
                                    let retry = ui.add_enabled(!busy, egui::Button::new("Retry"));
                                    observe("retry", &retry);
                                    if retry.clicked() {
                                        action = Some(Action::Retry);
                                    }
                                    let close = ui.button("Cancel");
                                    observe("cancel", &close);
                                    if close.clicked() {
                                        action = Some(Action::Close);
                                    }
                                });
                            }
                        });
                    });
            },
        );
        if paste_admitted {
            self.pending_widget_paste = None;
        }
        if paste_requested {
            self.request_widget_paste(context, child);
        }
        match action {
            Some(Action::Close) => self.close_git_setup(context),
            Some(Action::Retry) => {
                self.git_setup.start(self.workspace_root.clone(), context);
                context.request_repaint();
            }
            Some(Action::Apply) => {
                self.git_setup.apply(context);
                context.request_repaint();
            }
            Some(Action::SignInGitHub) => {
                self.git_setup.sign_in(context);
                context.request_repaint();
            }
            Some(Action::CancelGitHub) => {
                self.git_setup.cancel_github();
                context.request_repaint();
            }
            Some(Action::CreateGitHub) => {
                self.git_setup.create_github(context);
                context.request_repaint();
            }
            Some(Action::AttachGitHub) => {
                self.git_setup.attach_github(context, false);
                context.request_repaint();
            }
            Some(Action::ForgetGitHub) => {
                self.git_setup.attach_github(context, true);
                context.request_repaint();
            }
            None => {}
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn git_setup_inspection(&self, context: &egui::Context) -> serde_json::Value {
        let dialog = self.git_setup.dialog.as_ref();
        let child = scoped_child_viewport_id(context, SALT);
        let zoom = context.zoom_factor();
        let rect = context
            .input_for(child, |input| input.viewport().inner_rect)
            .map(|rect| {
                [
                    rect.left() * zoom,
                    rect.top() * zoom,
                    rect.right() * zoom,
                    rect.bottom() * zoom,
                ]
            });
        let github = dialog.map(|dialog| &dialog.github);
        let created = github.and_then(|github| github.created.as_ref());
        let archive = self.git_setup.archive.as_ref();
        let phase = self
            .git_setup
            .operation
            .pending()
            .map(|pending| pending.phase);
        let github_busy = self.git_setup.operation.is_running()
            && matches!(
                phase,
                Some(
                    Phase::SignIn | Phase::CreateGitHub | Phase::AttachGitHub | Phase::ForgetGitHub
                )
            );
        serde_json::json!({
            "visible": dialog.is_some(), "busy": self.git_setup.operation.is_running(),
            "loaded": dialog.is_some_and(|dialog| dialog.snapshot.is_some()),
            "initialized": dialog.and_then(|dialog| dialog.snapshot.as_ref()).is_some_and(|snapshot| snapshot.initialized),
            "workspace": dialog.map(|dialog| &dialog.workspace),
            "root": dialog.and_then(|dialog| dialog.snapshot.as_ref()).map(|snapshot| &snapshot.root),
            "branch": dialog.map(|dialog| &dialog.draft.branch),
            "author_set": dialog.is_some_and(|dialog| !dialog.draft.name.is_empty() && !dialog.draft.email.is_empty()),
            "origin_set": dialog.is_some_and(|dialog| !dialog.draft.origin.is_empty()),
            "name_fingerprint": dialog.map(|dialog| crate::desktop_test::fingerprint(&dialog.draft.name)),
            "email_fingerprint": dialog.map(|dialog| crate::desktop_test::fingerprint(&dialog.draft.email)),
            "error": dialog.and_then(|dialog| dialog.error.as_ref()),
            "viewport": format!("{child:?}"), "rect": rect, "zoom": zoom,
            "github": {
                "selected": github.is_some_and(|github| github.selected),
                "signed_in": github.is_some_and(|github| github.account.is_some()),
                "account_id": github.and_then(|github| github.account.as_ref()).map(|account| account.id),
                "account_login": github.and_then(|github| github.account.as_ref()).map(|account| &account.login),
                "name_fingerprint": github.map(|github| crate::desktop_test::fingerprint(&github.name)),
                "private": github.is_none_or(|github| github.private),
                "stage": match phase { Some(Phase::CreateGitHub) => "creating", Some(Phase::AttachGitHub) => "attaching", Some(Phase::ForgetGitHub) => "checking", _ => github.map_or("idle", |github| github.stage()) },
                "code_ready": github.is_some_and(|github| github.code().is_some()),
                "busy": github_busy,
                "open_requested": github.is_some_and(|github| github.opened_url.is_some()),
                "opened_url_fingerprint": github.and_then(|github| github.opened_url.as_ref()).map(|url| crate::desktop_test::fingerprint(url)),
                "created_url": created.filter(|created| created.confirmed()).map(|created| &created.html_url),
                "expected_url": created.filter(|created| !created.confirmed()).map(|created| &created.html_url),
                "uncertain": created.is_some_and(|created| !created.confirmed()),
                "attach_failed": created.is_some_and(|created| created.snapshot.is_err()),
            },
            "last_github_outcome": archive.map(|archive| serde_json::json!({"workspace":archive.workspace,"created_url":archive.created.confirmed().then_some(&archive.created.html_url),"expected_url":(!archive.created.confirmed()).then_some(&archive.created.html_url),"confirmed":archive.created.confirmed(),"attach_failed":archive.created.snapshot.is_err()})),
        })
    }
}

#[cfg(test)]
mod tests;
