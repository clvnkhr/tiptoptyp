//! Explorer mutations use the existing exclusive import worker. Copying a large
//! file or folder must never block painting, and completion belongs to its root.
use super::*;

pub(super) enum FileFollowUp {
    Open(PathBuf),
    Rename(PathBuf),
    RetryCreate(RenameDialog),
}

impl EditorApp {
    pub(super) fn delete_workspace_file(&mut self, path: PathBuf, context: &egui::Context) {
        if self.document_workflow.save_in_flight()
            || self.local_save.is_running()
            || self.workspace_switch.active()
            || self.process_close_pending()
        {
            self.notice = Some(Notice {
                message: "Finish the current file operation before deleting.".into(),
                kind: NoticeKind::Info,
            });
            return;
        }
        if self.is_overleaf_path(&path) {
            self.begin_overleaf_file_action(overleaf::files::Action::Delete(path));
            return;
        }
        match crate::workspace::WorkspaceRoot::open(&self.workspace_root)
            .and_then(|root| root.file(&path))
            .and_then(|file| {
                let path = file.path().to_owned();
                file.delete().map(|()| path)
            }) {
            Ok(path) => {
                let deletes_preview = self.designated_preview_path().as_ref() == Some(&path);
                let retired_preview = self.close_deleted_parked_tabs(&path, context);
                if deletes_preview && !retired_preview {
                    self.restart_tinymist_preserving_preview();
                    self.schedule_compile_now();
                }
                if self.document().path().as_ref() == Some(&path) {
                    self.reset_untitled_document();
                }
                self.refresh_workspace();
                self.notice = Some(Notice {
                    message: format!("Deleted {}", path.display()),
                    kind: NoticeKind::Success,
                });
            }
            Err(error) => {
                self.show_file_error(format!("Could not delete {}: {error}", path.display()))
            }
        }
    }

    pub(super) fn begin_workspace_new_file(&mut self, directory: PathBuf, tex: bool) {
        if self.document_flow_busy() || self.file_import.is_running() {
            return;
        }
        if self.is_overleaf_path(&directory) {
            self.begin_overleaf_file_action(overleaf::files::Action::NewFile(directory));
            return;
        }
        self.rename_dialog = Some(RenameDialog {
            path: directory,
            name: if tex || self.document().kind() == DocumentKind::Tex {
                "untitled.tex"
            } else {
                "untitled.typ"
            }
            .into(),
            focus: true,
            create: true,
            error: None,
            edit_events: Vec::new(),
        });
        self.rename_overlay_focus = ModalFocus::default();
    }

    pub(super) fn create_workspace_file(
        &mut self,
        mut dialog: RenameDialog,
        context: &egui::Context,
    ) {
        if self.is_overleaf_path(&dialog.path) {
            self.show_file_error("Create Overleaf files in Overleaf.".into());
            return;
        }
        let name = dialog.name.trim();
        let error = crate::workspace::validate_entry_name(name.as_ref())
            .err()
            .map(|error| error.to_string());
        if let Some(error) = error {
            dialog.error = Some(error);
            dialog.focus = true;
            self.rename_dialog = Some(dialog);
            return;
        }
        let root = self.workspace_root.clone();
        let name = name.to_owned();
        if let Err(error) = self
            .file_import
            .start_and_repaint("new-file", context, move || {
                let created = crate::workspace::WorkspaceRoot::open(&root)
                    .and_then(|root| root.directory(&dialog.path))
                    .and_then(|directory| directory.create_file(name.as_ref()));
                let path = match created {
                    Ok(path) => path,
                    Err(error) => {
                        let error = if error.kind() == std::io::ErrorKind::AlreadyExists {
                            format!("{name} already exists")
                        } else {
                            error.to_string()
                        };
                        dialog.error = Some(error.clone());
                        dialog.focus = true;
                        return Ok(FileImportResult {
                            workspace: root,
                            message: format!("Could not create {name}"),
                            imported: Vec::new(),
                            warning: Some(error),
                            follow_up: Some(FileFollowUp::RetryCreate(dialog)),
                        });
                    }
                };
                Ok(FileImportResult {
                    workspace: root,
                    message: format!("Created {}", path.display()),
                    imported: vec![path.clone()],
                    warning: None,
                    follow_up: Some(FileFollowUp::Open(path)),
                })
            })
        {
            self.show_file_error(error);
        }
    }

    pub(super) fn duplicate_workspace_entry(&mut self, path: PathBuf, context: &egui::Context) {
        if self.document_flow_busy() || self.file_import.is_running() {
            return;
        }
        if self.is_overleaf_path(&path) {
            self.begin_overleaf_file_action(overleaf::files::Action::Duplicate(path));
            return;
        }
        let root = self.workspace_root.clone();
        if let Err(error) =
            self.file_import
                .start_and_repaint("duplicate-file", context, move || {
                    let destination = crate::workspace::WorkspaceRoot::open(&root)
                        .and_then(|root| root.duplicate(&path))
                        .map_err(|error| {
                            format!("Could not duplicate {}: {error}", path.display())
                        })?;
                    Ok(FileImportResult {
                        workspace: root,
                        message: format!("Duplicated to {}", destination.display()),
                        imported: vec![destination.clone()],
                        warning: None,
                        follow_up: Some(FileFollowUp::Rename(destination)),
                    })
                })
        {
            self.show_file_error(error);
        }
    }
}

pub(super) fn file_dialog_footer(ui: &mut egui::Ui, primary: &str) -> Option<AppModalChoice> {
    let mut choice = None;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let submit = ui.add(egui::Button::new(primary).fill(ui.visuals().selection.bg_fill));
        let cancel = ui.button("Cancel");
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        {
            crate::desktop_test::observe("rename.submit", &submit);
            crate::desktop_test::observe("rename.cancel", &cancel);
        }
        if submit.clicked() {
            choice = Some(AppModalChoice::Primary);
        } else if cancel.clicked() {
            choice = Some(AppModalChoice::Cancel);
        }
    });
    choice
}

#[cfg(test)]
mod tests;
