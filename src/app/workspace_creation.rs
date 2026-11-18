//! Explicit, asynchronous workspace creation in the existing folder chooser.
use super::*;
use crate::worker::{ExclusiveJob, OperationSummary};

pub(super) const SALT: &str = "tiptoptyp-workspace-chooser";

#[derive(Default)]
pub(super) struct State {
    pub(super) draft: Option<Draft>,
    phase: Phase,
    pub(super) edit_events: Vec<egui::Event>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Picking(crate::workflow::PendingDialog<()>),
    Creating(ExclusiveJob<Created>),
}
pub(super) struct Draft {
    pub(super) name: String,
    pub(super) parent: String,
    pub(super) use_overleaf: bool,
    pub(super) error: Option<String>,
    focus: bool,
}

struct Created {
    root: PathBuf,
    origin: PathBuf,
    use_overleaf: bool,
}
impl OperationSummary for Created {
    fn completion_summary(&self) -> String {
        format!("Created workspace {}", self.root.display())
    }
}
impl State {
    pub(super) fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }
    fn poll_picker(&mut self, context: &egui::Context) -> crate::workflow::DialogPoll<()> {
        if !matches!(self.phase, Phase::Picking(_)) {
            return crate::workflow::DialogPoll::Idle;
        }
        let Phase::Picking(picker) = std::mem::take(&mut self.phase) else {
            unreachable!()
        };
        let mut picker = Some(picker);
        let outcome = crate::workflow::poll_dialog(&mut picker, context);
        if let Some(picker) = picker {
            self.phase = Phase::Picking(picker);
        }
        outcome
    }
    fn poll_created(&mut self) -> LatestJobPoll<Created> {
        let Phase::Creating(job) = &mut self.phase else {
            return LatestJobPoll::Idle;
        };
        let outcome = job.poll();
        if matches!(outcome, LatestJobPoll::Ready(_) | LatestJobPoll::Failed(_)) {
            self.phase = Phase::Idle;
        }
        outcome
    }
}
impl Draft {
    fn request(&self) -> Result<(PathBuf, String), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Enter a workspace name.".into());
        }
        crate::workspace::validate_entry_name(name.as_ref()).map_err(|e| e.to_string())?;
        let parent = PathBuf::from(&self.parent);
        if !parent.is_absolute() {
            return Err("Choose an existing parent folder with an absolute path.".into());
        }
        Ok((parent, name.into()))
    }
}

#[derive(Default)]
pub(super) struct Action {
    pub(super) choose_parent: bool,
    pub(super) create: bool,
    pub(super) cancel: bool,
}

pub(super) fn show(ui: &mut egui::Ui, draft: &mut Draft, busy: bool) -> Action {
    let mut action = Action::default();
    ui.strong("New workspace");
    ui.add_enabled_ui(!busy, |ui| {
        let name_label = ui.label("Name");
        let name = ui.add(
            egui::TextEdit::singleline(&mut draft.name)
                .id_salt("new-workspace-name")
                .desired_width(f32::INFINITY),
        );
        let name = name.labelled_by(name_label.id);
        observe("name", &name);
        if draft.focus {
            name.request_focus();
            draft.focus = false;
        }
        let mut changed = name.changed();
        let parent_label = ui.label("Parent folder");
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 90.0).max(40.0);
            let parent = ui.add(
                egui::TextEdit::singleline(&mut draft.parent)
                    .id_salt("new-workspace-parent")
                    .desired_width(width),
            );
            let parent = parent.labelled_by(parent_label.id);
            observe("parent", &parent);
            changed |= parent.changed();
            let choose = ui.button("Choose…");
            observe("choose_parent", &choose);
            action.choose_parent = choose.clicked();
        });
        let overleaf = ui.checkbox(&mut draft.use_overleaf, "Use for an Overleaf project");
        observe("overleaf", &overleaf);
        changed |= overleaf.changed();
        if changed {
            draft.error = None;
        }
        if let Some(error) = &draft.error {
            ui.colored_label(error_color(ui.ctx()), error);
        }
        ui.add_space(theme::SPACE.control);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let create = ui.button("Create workspace");
            observe("create", &create);
            action.create = create.clicked();
            let cancel = ui.button("Cancel");
            observe("cancel", &cancel);
            action.cancel = cancel.clicked();
        });
    });
    action.create |= !busy && ui.input(|input| input.key_pressed(egui::Key::Enter));
    if busy {
        ui.label("Preparing workspace…");
    }
    action
}

fn observe(name: &str, response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(&format!("workspace.{name}"), response);
    let _ = (name, response);
}

impl EditorApp {
    pub(super) fn begin_workspace_creation(&mut self) {
        if self.workspace_creation.busy() || self.document_flow_busy() {
            return;
        }
        let local = if self.is_overleaf_path(&self.workspace_root) {
            self.settings
                .existing_recent_workspaces()
                .into_iter()
                .find(|path| !self.is_overleaf_path(path))
        } else {
            Some(self.workspace_root.clone())
        };
        let parent = local
            .as_deref()
            .and_then(Path::parent)
            .unwrap_or(&self.workspace_root)
            .to_string_lossy()
            .into_owned();
        self.workspace_creation.draft = Some(Draft {
            name: String::new(),
            parent,
            use_overleaf: false,
            error: None,
            focus: true,
        });
    }

    pub(super) fn submit_workspace_creation(&mut self, context: &egui::Context) {
        if self.workspace_creation.busy() {
            return;
        }
        let Some(draft) = &mut self.workspace_creation.draft else {
            return;
        };
        let (parent, name) = match draft.request() {
            Ok(request) => request,
            Err(error) => {
                draft.error = Some(error);
                return;
            }
        };
        let use_overleaf = draft.use_overleaf;
        let origin = self.workspace_root.clone();
        let mut job = ExclusiveJob::default();
        match job.start_and_repaint("new-workspace", context, move || {
            let root =
                crate::workspace::create_workspace(&parent, name.as_ref()).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        format!("{name} already exists. Choose another workspace name.")
                    } else {
                        format!("Could not create workspace: {error}")
                    }
                })?;
            Ok(Created {
                root,
                origin,
                use_overleaf,
            })
        }) {
            Ok(()) => self.workspace_creation.phase = Phase::Creating(job),
            Err(error) => draft.error = Some(error),
        }
    }

    pub(super) fn choose_workspace_parent(&mut self, context: &egui::Context) {
        if self.workspace_creation.busy() {
            return;
        }
        let Some(draft) = &self.workspace_creation.draft else {
            return;
        };
        let mut dialog = AsyncFileDialog::new()
            .set_title("New workspace parent folder")
            .set_directory(&draft.parent);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(parent) =
            crate::native_window::for_viewport(context, scoped_child_viewport_id(context, SALT))
        {
            dialog = dialog.set_parent(&parent);
        }
        self.workspace_creation.phase = Phase::Picking(crate::workflow::PendingDialog::new(
            (),
            dialog.pick_folder(),
        ));
    }

    pub(super) fn poll_workspace_creation(&mut self, context: &egui::Context) {
        if let crate::workflow::DialogPoll::Ready {
            selection: Some(folder),
            ..
        } = self.workspace_creation.poll_picker(context)
            && let Some(draft) = &mut self.workspace_creation.draft
        {
            draft.parent = folder.path().to_string_lossy().into_owned();
            draft.error = None;
        }
        // A save may start after the form opens. Keep the completed creation
        // owned by its job until that save retires its account/document ticket.
        if self.document_workflow.save_in_flight() || self.local_save.is_running() {
            return;
        }
        match self.workspace_creation.poll_created() {
            LatestJobPoll::Ready(created) => {
                self.workspace_creation.draft = None;
                self.workspace_creation.edit_events.clear();
                self.remember_workspace(&created.root);
                self.notice = Some(Notice {
                    message: created.completion_summary(),
                    kind: NoticeKind::Success,
                });
                if self.workspace_root != created.origin {
                    return;
                }
                self.workspace_chooser_visible = false;
                crate::window_host::return_focus(context);
                if created.use_overleaf {
                    self.stage_overleaf_workspace(&created.root, context);
                } else {
                    self.queue_open_path(created.root);
                }
            }
            LatestJobPoll::Failed(error) => {
                if let Some(draft) = &mut self.workspace_creation.draft {
                    draft.error = Some(error);
                }
            }
            _ => {}
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn workspace_creation_inspection(&self) -> serde_json::Value {
        let draft = self.workspace_creation.draft.as_ref();
        serde_json::json!({
            "visible": self.workspace_chooser_visible && draft.is_some(),
            "busy": self.workspace_creation.busy(),
            "phase": match self.workspace_creation.phase { Phase::Idle => "idle", Phase::Picking(_) => "picking", Phase::Creating(_) => "creating" },
            "use_overleaf": draft.is_some_and(|draft| draft.use_overleaf),
            "name_fingerprint": draft.map(|draft| crate::desktop_test::fingerprint(&draft.name)),
            "parent_fingerprint": draft.map(|draft| crate::desktop_test::fingerprint(&draft.parent)),
            "error": draft.and_then(|draft| draft.error.as_ref()).map(|error| crate::desktop_test::fingerprint(error))
        })
    }
}

#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod tests;
