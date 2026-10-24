//! Reopening a managed folder always goes through its authoritative project.
use super::*;
use crate::overleaf::StoredProject;
use crate::worker::LatestJob;

#[derive(Default)]
pub(super) struct State {
    discovery: LatestJob<Option<StoredProject>>,
    pending: Option<Pending>,
    known: Vec<StoredProject>,
}
struct Pending {
    path: PathBuf,
    owner: Option<u64>,
    key: Option<DocumentKey>,
    workspace: PathBuf,
}

/// No filesystem reads: also recognize previous default folders after moving
/// a project's copy, so those folders never become ordinary editable projects.
pub(in crate::app) fn known_project_for_path(
    settings: &AppSettings,
    path: &Path,
) -> Option<(ProjectUrl, PathBuf)> {
    // Ordinary editor frames never parse project URLs or hash origins. Only
    // paths containing an actual namespaced project-folder shape can match.
    let candidates: Vec<_> = path
        .ancestors()
        .filter_map(|root| {
            let name = root.file_name()?.to_str()?;
            (name.len() == 41
                && name.as_bytes()[16] == b'-'
                && name[..16].bytes().all(|byte| byte.is_ascii_hexdigit())
                && name[17..].bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| &name[17..])
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let base = cache_base().ok()?;
    settings
        .overleaf_projects
        .iter()
        .chain(settings.overleaf_storage.keys())
        .filter(|url| {
            url.rsplit('/')
                .next()
                .is_some_and(|id| candidates.contains(&id))
        })
        .filter_map(|url| ProjectUrl::parse(url).ok())
        .find_map(|project| {
            let default = project.mirror_root(&base);
            if path.starts_with(&default) {
                return Some((project, default));
            }
            let directory = settings
                .overleaf_storage
                .get(project.url())?
                .directory
                .as_ref()?;
            let root = project.mirror_root(Path::new(directory));
            path.starts_with(&root).then_some((project, root))
        })
}

fn temporary_root(path: &Path) -> Option<&Path> {
    path.ancestors().find(|root| {
        root.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("tiptoptyp-overleaf-"))
    })
}

/// Only explicit opens probe metadata. Descriptor parsing and ancestor
/// validation remain on a worker; normal files keep the synchronous open path.
fn descriptor_hint(path: &Path) -> bool {
    path.ancestors().take(128).any(|root| {
        root.join(".tiptoptyp/overleaf-project.json")
            .symlink_metadata()
            .is_ok()
    })
}

impl EditorApp {
    pub(in crate::app) fn is_overleaf_path(&self, path: &Path) -> bool {
        self.overleaf
            .connection
            .as_ref()
            .is_some_and(|c| path.starts_with(&c.root))
            || self
                .overleaf
                .reopen
                .known
                .iter()
                .any(|p| path.starts_with(&p.root))
            || is_mirror_path(path)
            || temporary_root(path).is_some()
            || known_project_for_path(
                self.pending_settings.as_ref().unwrap_or(&self.settings),
                path,
            )
            .is_some()
    }

    pub(in crate::app) fn overleaf_reopen_locked(&self) -> bool {
        self.document().path().as_ref().is_some_and(|path| {
            self.overleaf
                .reopen
                .pending
                .as_ref()
                .is_some_and(|pending| pending.path == *path)
        })
    }

    fn overleaf_project_for_path(&self, path: &Path) -> Option<(ProjectUrl, PathBuf)> {
        self.overleaf
            .connection
            .as_ref()
            .filter(|c| path.starts_with(&c.root))
            .map(|c| (c.project.clone(), c.root.clone()))
            .or_else(|| {
                self.overleaf
                    .reopen
                    .known
                    .iter()
                    .find(|project| path.starts_with(&project.root))
                    .map(|stored| (stored.project.clone(), stored.root.clone()))
            })
            .or_else(|| {
                known_project_for_path(
                    self.pending_settings.as_ref().unwrap_or(&self.settings),
                    path,
                )
            })
    }

    /// True means handled/deferred, never that the old disk copy was opened.
    pub(in crate::app) fn intercept_overleaf_path(
        &mut self,
        path: &Path,
        context: &egui::Context,
    ) -> bool {
        let path = canonical_or_absolute(path);
        if self.overleaf.admitted_path.as_ref() == Some(&path) {
            return false;
        }
        if self
            .overleaf
            .reopen
            .pending
            .as_ref()
            .is_some_and(|pending| pending.path == path)
        {
            return true;
        }
        // A subsequent explicit open owns navigation, even when it is local.
        self.overleaf.reopen.pending = None;
        self.overleaf.reopen.discovery.supersede();
        if let Some((project, root)) = self.overleaf_project_for_path(&path) {
            let relative = path
                .strip_prefix(&root)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty() && !path.is_dir())
                .map(Path::to_path_buf);
            let current = self.overleaf.connection.as_ref().is_some_and(|c| {
                c.project == project && c.root == root && c.online && !c.hydration.pending
            });
            if current && relative.is_some() && self.open_overleaf_path(&path) {
                return true;
            }
            if let Err(error) = self.open_overleaf_project_url(project.url(), relative, context) {
                self.show_file_error(error);
            }
            return true;
        }
        if descriptor_hint(&path) {
            let request = Pending {
                path: path.clone(),
                owner: self.tabs.active_id(),
                key: (!self.tabs.is_empty()).then(|| self.document().key()),
                workspace: self.workspace_root.clone(),
            };
            match self.overleaf.reopen.discovery.start_and_repaint(
                "Locate Overleaf project",
                context,
                move || StoredProject::from_path(&path),
            ) {
                Ok(()) => self.overleaf.reopen.pending = Some(request),
                Err(error) => self.show_file_error(error),
            }
            return true;
        }
        if self.is_overleaf_path(&path) {
            self.show_file_error("Open this Overleaf project from the recent project list to reconnect before editing its files.".into());
            return true;
        }
        false
    }

    pub(in crate::app) fn poll_overleaf_reopen(&mut self, context: &egui::Context) {
        let result = match self.overleaf.reopen.discovery.poll() {
            LatestJobPoll::Ready(stored) => Ok(stored),
            LatestJobPoll::Failed(error) => Err(error),
            LatestJobPoll::Idle | LatestJobPoll::Pending => return,
        };
        let Some(request) = self.overleaf.reopen.pending.take() else {
            return;
        };
        let owned = request.owner == self.tabs.active_id()
            && request.key == (!self.tabs.is_empty()).then(|| self.document().key())
            && request.workspace == self.workspace_root;
        match result {
            Ok(Some(stored)) => {
                self.overleaf.reopen.known.retain(|old| old.root != stored.root);
                self.overleaf.reopen.known.push(stored.clone());
                if self.overleaf.reopen.known.len() > 64 {
                    self.overleaf.reopen.known.remove(0);
                }
                self.prune_overleaf_workspace_history();
                if !owned { return; }
                if stored.persistent {
                    let Some(base) = stored.root.parent() else { return; };
                    if stored.project.mirror_root(base) != stored.root {
                        self.show_file_error("The Overleaf working-copy folder was renamed. Open the project from its URL and choose a local-copy folder.".into());
                        return;
                    }
                    let mut settings = self.pending_settings.clone().unwrap_or_else(|| self.settings.clone());
                    let preference = crate::settings::OverleafStoragePreference {
                        keep_local_copy: true,
                        directory: Some(base.to_string_lossy().into_owned()),
                    };
                    if let Err(error) = settings.set_overleaf_storage_preference(stored.project.url(), preference) {
                        self.show_file_error(error);
                        return;
                    }
                    self.queue_settings(settings, context);
                }
                let relative = request.path.strip_prefix(&stored.root).ok()
                    .filter(|relative| !relative.as_os_str().is_empty() && !request.path.is_dir())
                    .map(Path::to_path_buf);
                if let Err(error) = self.open_overleaf_project_url(stored.project.url(), relative, context) {
                    self.show_file_error(error);
                }
            }
            Ok(None) if owned => self.show_file_error("The Overleaf project descriptor is no longer available. Reopen the project from its URL.".into()),
            Err(error) if owned => self.show_file_error(error),
            _ => {}
        }
    }

    pub(in crate::app) fn prune_overleaf_workspace_history(&mut self) {
        let roots: Vec<_> = self
            .settings
            .recent_workspaces
            .iter()
            .map(PathBuf::from)
            .filter(|root| self.is_overleaf_path(root))
            .collect();
        for root in roots {
            self.forget_workspace(&root);
        }
        let keys: Vec<_> = self
            .settings
            .last_opened_files
            .keys()
            .filter(|root| self.is_overleaf_path(Path::new(root)))
            .cloned()
            .collect();
        for key in keys {
            self.settings.last_opened_files.remove(&key);
            if let Some(settings) = &mut self.pending_settings {
                settings.last_opened_files.remove(&key);
            }
        }
    }

    pub(in crate::app) fn overleaf_project_label(&self, url: &str) -> String {
        if let Some(connection) = self
            .overleaf
            .connection
            .as_ref()
            .filter(|c| c.project.url() == url)
            && let Some(tree) = connection.model.tree()
        {
            return format!("Overleaf · {}", tree.name);
        }
        ProjectUrl::parse(url).map_or_else(
            |_| "Overleaf".into(),
            |project| {
                format!(
                    "Overleaf · {} · {}",
                    project.origin().trim_start_matches("https://"),
                    &project.id()[16..]
                )
            },
        )
    }

    pub(in crate::app) fn overleaf_workspace_label(&self) -> Option<String> {
        self.overleaf_project_for_path(&self.workspace_root)
            .map(|(project, _)| self.overleaf_project_label(project.url()))
            .or_else(|| temporary_root(&self.workspace_root).map(|_| "Overleaf".into()))
    }

    /// Physical temporary paths are disclosed only by hovering the root.
    pub(in crate::app) fn document_location_label(&self, path: &Path) -> String {
        let temporary = self
            .overleaf
            .connection
            .as_ref()
            .filter(|c| {
                path.starts_with(&c.root) && c.storage.as_ref().is_some_and(|s| !s.persistent())
            })
            .map(|c| c.root.as_path())
            .or_else(|| {
                self.overleaf
                    .reopen
                    .known
                    .iter()
                    .find(|p| !p.persistent && path.starts_with(&p.root))
                    .map(|p| p.root.as_path())
            })
            .or_else(|| temporary_root(path));
        if let Some(root) = temporary {
            return path
                .strip_prefix(root)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .map_or_else(
                    || "Overleaf".into(),
                    |relative| format!("Overleaf · {}", relative.display()),
                );
        }
        path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const URL: &str = "https://www.overleaf.com/project/0123456789abcdef01234567";

    #[test]
    fn configured_and_old_default_copies_are_managed_even_after_disabling_local_storage() {
        let project = ProjectUrl::parse(URL).unwrap();
        let custom = PathBuf::from("/tmp/overleaf-reopen-choice");
        let mut settings = AppSettings::default();
        settings
            .set_overleaf_storage_preference(
                URL,
                crate::settings::OverleafStoragePreference {
                    keep_local_copy: false,
                    directory: Some(custom.to_string_lossy().into_owned()),
                },
            )
            .unwrap();
        for base in [custom.clone(), cache_base().unwrap()] {
            let root = project.mirror_root(&base);
            assert_eq!(
                known_project_for_path(&settings, &root.join("main.tex")),
                Some((project.clone(), root))
            );
        }
        assert!(known_project_for_path(&settings, &custom.join("ordinary.tex")).is_none());
    }

    #[test]
    fn all_managed_open_entrypoints_reject_stale_disk_content_without_authentication() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        let path = root.join("main.tex");
        fs::write(&path, "stale disk copy").unwrap();
        app.document_mut().replace_loaded_unprojected(
            "retained native draft".into(),
            path.clone(),
            DocumentKind::Tex,
            None,
        );
        app.overleaf.connection = Some(Connection {
            project: ProjectUrl::parse(URL).unwrap(),
            root: root.clone(),
            model: Model::new(1),
            status: "Signed out".into(),
            online: false,
            storage: None,
            hydration: Hydration::waiting(),
        });
        let key = app.document().key();
        let tabs = app.tabs.len();
        assert!(!app.open_tab_path(path.clone(), &context));
        assert!(!app.load_path(path.clone()));
        assert!(!app.open_git_file_diff_path(path, &context));
        app.finish_open_folder_selection(root);
        assert_eq!(app.document().source(), "retained native draft");
        assert_eq!(app.document().key(), key);
        assert_eq!(app.tabs.len(), tabs);
        assert!(app.overleaf.error.is_some() || app.notice.is_some());
    }

    #[test]
    fn descriptor_discovery_never_steals_an_editor_that_changed_while_it_was_loading() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let project = ProjectUrl::parse(URL).unwrap();
        let lease = crate::overleaf::StorageLease::prepare(
            &project,
            &crate::overleaf::StorageOptions {
                keep_local_copy: true,
                directory: Some(base.join("copies")),
            },
            &base,
        )
        .unwrap();
        let remote = lease.root().join("main.tex");
        fs::write(&remote, "stale remote copy").unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, base.clone());
        app.document_mut().replace_unprojected_untitled("local");
        assert!(app.intercept_overleaf_path(&remote, &context));
        assert!(app.overleaf.reopen.discovery.is_running());
        app.document_mut().edit(CCursorRange::default(), |source| {
            source.push_str(" new edit")
        });
        let key = app.document().key();
        app.overleaf
            .reopen
            .discovery
            .wait_until_ready_for_test()
            .unwrap();
        app.poll_overleaf_reopen(&context);
        assert_eq!(app.document().source(), "local new edit");
        assert_eq!(app.document().key(), key);
        assert!(app.overleaf.connection.is_none());
        assert!(!app.overleaf.storage_job.is_running());
        assert!(app.is_overleaf_path(&remote));
    }

    #[test]
    fn ordinary_files_open_immediately_and_temporary_paths_are_only_logical_labels() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let path = base.join("local.tex");
        fs::write(&path, "ordinary source").unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, base.clone());
        assert!(app.open_tab_path(path, &context));
        assert_eq!(app.document().source(), "ordinary source");
        assert!(!app.overleaf.reopen.discovery.is_running());
        let temporary = base.join("tiptoptyp-overleaf-private-test");
        assert_eq!(
            app.document_location_label(&temporary.join("sub/main.tex")),
            "Overleaf · sub/main.tex"
        );
        app.workspace_root = temporary;
        assert_eq!(app.overleaf_workspace_label().as_deref(), Some("Overleaf"));
    }

    #[test]
    fn managed_roots_and_files_are_pruned_from_local_history_while_local_roots_remain() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, base.clone());
        app.settings.remember_overleaf_project(URL).unwrap();
        let managed = ProjectUrl::parse(URL)
            .unwrap()
            .mirror_root(&cache_base().unwrap());
        app.settings.recent_workspaces =
            vec![managed.display().to_string(), base.display().to_string()];
        app.settings.last_opened_files.insert(
            managed.display().to_string(),
            managed.join("main.tex").display().to_string(),
        );
        app.prune_overleaf_workspace_history();
        assert_eq!(app.settings.recent_workspaces, [base.display().to_string()]);
        assert!(
            !app.settings
                .last_opened_files
                .contains_key(&managed.display().to_string())
        );
        assert_eq!(app.settings.overleaf_projects, [URL]);
    }
}
