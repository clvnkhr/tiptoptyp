//! A demo is created only after the user chooses a real file destination.
use super::*;
use std::io::Write as _;

pub(super) const SOURCE: &str = r##"#set page(paper: "a4", margin: 2.2cm)
#set text(size: 11pt)
#set heading(numbering: "1.")

= Welcome to tiptoptyp

Edit this document on the left. The PDF preview updates as you type.

#let accent = rgb("#4f8cff")
#text(fill: accent, weight: "bold")[A small, fast Typst and LaTeX workspace.]

== Math and code

Inline math is highlighted too: $ integral_0^infinity e^(-x) dif x = 1 $.

#for item in ("Native UI", "Live PDF", "Fast rebuilds") [
  - #item
]
"##;

#[derive(Default)]
pub(super) struct State {
    phase: Phase,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Picking(crate::workflow::PendingDialog<PathBuf>),
    Creating(crate::worker::ExclusiveJob<Created>),
}
impl State {
    pub(super) fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    fn poll_picker(&mut self, context: &egui::Context) -> crate::workflow::DialogPoll<PathBuf> {
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
    fn poll_created(&mut self) -> crate::worker::LatestJobPoll<Created> {
        let Phase::Creating(job) = &mut self.phase else {
            return crate::worker::LatestJobPoll::Idle;
        };
        let outcome = job.poll();
        if matches!(
            outcome,
            crate::worker::LatestJobPoll::Ready(_) | crate::worker::LatestJobPoll::Failed(_)
        ) {
            self.phase = Phase::Idle;
        }
        outcome
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspection(&self) -> serde_json::Value {
        serde_json::json!({"phase":match self.phase { Phase::Idle => "idle", Phase::Picking(_) => "picking", Phase::Creating(_) => "creating" }, "picking": matches!(self.phase, Phase::Picking(_)), "busy": self.busy()})
    }
}

struct Created {
    path: PathBuf,
    workspace: PathBuf,
}

impl crate::worker::OperationSummary for Created {
    fn completion_summary(&self) -> String {
        format!("Created demo file {}", self.path.display())
    }
}

/// Publish complete demo contents without replacing any existing destination.
/// The owned staging file is removed automatically on any pre-publication error.
fn create_file(path: &Path) -> Result<(), String> {
    let name = path.file_name().ok_or("Choose a name for the demo file.")?;
    let parent = path
        .parent()
        .ok_or("Choose a folder for the demo file.")?
        .canonicalize()
        .map_err(|error| format!("Could not open the demo folder: {error}"))?;
    if crate::overleaf::StoredProject::from_path(&parent)?.is_some() {
        return Err("Choose a local folder outside the Overleaf working copy.".into());
    }
    let path = parent.join(name);
    let mut staging = tempfile::NamedTempFile::new_in(&parent)
        .map_err(|error| format!("Could not prepare the demo file: {error}"))?;
    staging
        .write_all(SOURCE.as_bytes())
        .and_then(|()| staging.as_file().sync_all())
        .map_err(|error| format!("Could not write the demo file: {error}"))?;
    staging.persist_noclobber(&path).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            "A file already exists at that destination. Choose a new name for the demo file.".into()
        } else {
            format!("Could not create the demo file: {}", error.error)
        }
    })?;
    Ok(())
}

impl EditorApp {
    pub(super) fn begin_demo_file(&mut self, frame: Option<&eframe::Frame>) {
        if self.document_flow_busy() || self.demo.busy() || self.snapshot_scene.is_some() {
            return;
        }
        let Some(dialog) = self.native_file_dialog(frame) else {
            return;
        };
        let mut dialog = dialog
            .set_title("Make Demo File")
            .set_file_name("tiptoptyp-demo.typ")
            .add_filter("Typst document", &["typ"]);
        if let Some(directory) = self.current_directory() {
            dialog = dialog.set_directory(directory);
        }
        self.demo.phase = Phase::Picking(crate::workflow::PendingDialog::new(
            self.workspace_root.clone(),
            dialog.save_file(),
        ));
    }

    fn submit_demo_file(&mut self, workspace: PathBuf, path: PathBuf, context: &egui::Context) {
        if self.demo.busy() {
            return;
        }
        let path = crate::workflow::source_save_path(path, DocumentKind::Typst);
        if self.is_overleaf_path(&path) {
            self.show_file_error("Choose a local folder outside the Overleaf working copy.".into());
            return;
        }
        let mut job = crate::worker::ExclusiveJob::default();
        match job.start_and_repaint("Make demo file", context, move || {
            create_file(&path)?;
            Ok(Created { path, workspace })
        }) {
            Ok(()) => self.demo.phase = Phase::Creating(job),
            Err(error) => self.show_file_error(error),
        }
    }

    pub(super) fn poll_demo_file(&mut self, context: &egui::Context) {
        if let crate::workflow::DialogPoll::Ready {
            request,
            selection: Some(file),
        } = self.demo.poll_picker(context)
        {
            self.submit_demo_file(request, file.path().to_path_buf(), context);
        }
        match self.demo.poll_created() {
            crate::worker::LatestJobPoll::Ready(created) => {
                self.notice = Some(Notice {
                    message: crate::worker::OperationSummary::completion_summary(&created),
                    kind: NoticeKind::Success,
                });
                if self.workspace_root == created.workspace {
                    // The file already exists. Ordinary open/replacement rules
                    // protect any source edited while the save panel was open.
                    self.queued_open_requests.push_back(created.path);
                }
            }
            crate::worker::LatestJobPoll::Failed(error) => self.show_file_error(error),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_demo_creation_publishes_complete_named_contents_and_never_replaces_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("demo.typ");
        create_file(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), SOURCE);
        fs::write(&path, "Existing user document").unwrap();
        assert!(create_file(&path).unwrap_err().contains("already exists"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "Existing user document");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn demo_creation_rejects_an_unregistered_managed_root_before_publishing() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let project = crate::overleaf::ProjectUrl::parse(
            "https://www.overleaf.com/project/0123456789abcdef01234567",
        )
        .unwrap();
        let lease = crate::overleaf::StorageLease::prepare(
            &project,
            &crate::overleaf::StorageOptions {
                keep_local_copy: true,
                directory: Some(base.join("stored-projects")),
            },
            &base,
        )
        .unwrap();
        let nested = lease.root().join("notes");
        fs::create_dir(&nested).unwrap();
        let path = nested.join("demo.typ");
        assert!(
            create_file(&path)
                .unwrap_err()
                .contains("Overleaf working copy")
        );
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&nested).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn demo_creation_resolves_a_parent_alias_before_checking_stored_project_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let project = crate::overleaf::ProjectUrl::parse(
            "https://www.overleaf.com/project/0123456789abcdef01234567",
        )
        .unwrap();
        let lease = crate::overleaf::StorageLease::prepare(
            &project,
            &crate::overleaf::StorageOptions {
                keep_local_copy: true,
                directory: Some(base.join("stored-projects")),
            },
            &base,
        )
        .unwrap();
        let alias = base.join("ordinary-looking-folder");
        std::os::unix::fs::symlink(lease.root(), &alias).unwrap();
        assert!(
            create_file(&alias.join("demo.typ"))
                .unwrap_err()
                .contains("Overleaf working copy")
        );
        assert!(!lease.root().join("demo.typ").exists());
        assert!(
            fs::symlink_metadata(alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn cancelling_demo_destination_preserves_the_workspace_and_exact_source_owner() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("Retained source");
        let key = app.document().key();
        let tabs = app.tabs.ids().collect::<Vec<_>>();
        app.demo.phase = Phase::Picking(crate::workflow::PendingDialog::new(
            directory.path().into(),
            async { None },
        ));
        app.poll_demo_file(&context);
        assert!(!app.demo.busy());
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), "Retained source");
        assert_eq!(app.tabs.ids().collect::<Vec<_>>(), tabs);
        assert!(app.queued_open_requests.is_empty());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn creating_demo_queues_an_ordinary_file_open_and_never_replaces_the_dirty_source() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        app.document_mut().replace_unprojected_untitled("");
        app.document_mut().edit(CCursorRange::default(), |source| {
            source.push_str("Unsaved notes")
        });
        let key = app.document().key();
        let tabs = app.tabs.ids().collect::<Vec<_>>();
        let path = root.join("demo.typ");
        app.submit_demo_file(root, path.clone(), &context);
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.demo.busy() {
            app.poll_demo_file(&context);
            assert!(Instant::now() < deadline, "demo worker did not finish");
            std::thread::yield_now();
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), SOURCE);
        assert_eq!(app.tabs.ids().collect::<Vec<_>>(), tabs);
        app.process_open_requests(&context);
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), "Unsaved notes");
        assert!(app.document_workflow.modal().is_none());
        assert!(matches!(
            app.document_workflow.take_action().map(|pending| pending.action),
            Some(DeferredDocumentAction::LoadPath(selected)) if selected == path
        ));
        assert_eq!(app.tabs.ids().collect::<Vec<_>>(), tabs);
    }
}
