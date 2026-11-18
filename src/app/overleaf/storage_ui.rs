//! Compact storage choices; connecting and folder dialogs remain controller work.
use super::{Connection, STORAGE_CHANGE_OPEN_TABS, State};
use crate::settings::OverleafStoragePreference;
use eframe::egui;
use std::path::Path;

#[derive(Clone, Debug)]
pub(super) struct StorageDraft {
    pub(super) project_url: Option<String>,
    pub(super) keep_local_copy: bool,
    pub(super) directory: String,
}

impl StorageDraft {
    pub(super) fn new(url: &str, preference: &OverleafStoragePreference) -> Self {
        Self {
            project_url: Some(url.into()),
            keep_local_copy: preference.keep_local_copy,
            directory: preference.directory.clone().unwrap_or_default(),
        }
    }

    pub(super) fn unbound(preference: &OverleafStoragePreference) -> Self {
        Self {
            project_url: None,
            keep_local_copy: preference.keep_local_copy,
            directory: preference.directory.clone().unwrap_or_default(),
        }
    }

    pub(super) fn raw_preference(&self) -> OverleafStoragePreference {
        OverleafStoragePreference {
            keep_local_copy: self.keep_local_copy,
            directory: (!self.directory.trim().is_empty()).then(|| self.directory.clone()),
        }
    }

    pub(super) fn preference(&self) -> Result<OverleafStoragePreference, String> {
        let preference = self.raw_preference();
        match preference.normalized() {
            Ok(preference) => Ok(preference),
            // An unfinished local-folder edit is irrelevant in temporary mode.
            // Keep it in the draft so toggling back can show the correction.
            Err(_) if !self.keep_local_copy => Ok(OverleafStoragePreference {
                keep_local_copy: false,
                directory: None,
            }),
            Err(error) => Err(error),
        }
    }
}

impl State {
    pub(super) fn ensure_storage_draft(
        &mut self,
        project: Option<&crate::overleaf::ProjectUrl>,
        settings: &crate::settings::AppSettings,
    ) -> Result<(), String> {
        if self.storage_draft.as_ref().is_none_or(|draft| {
            draft.project_url.as_deref() != project.map(|project| project.url())
        }) {
            let preference = match &self.pending_storage {
                Some(preference) => preference.clone(),
                None => match project {
                    Some(project) => settings.overleaf_storage_preference(project.url())?,
                    None => {
                        self.storage_draft = None;
                        return Ok(());
                    }
                },
            };
            self.storage_draft = Some(match project {
                Some(project) => StorageDraft::new(project.url(), &preference),
                None => StorageDraft::unbound(&preference),
            });
        }
        Ok(())
    }

    pub(super) fn accept_pending_storage(&mut self, prepared: &Option<OverleafStoragePreference>) {
        // An older preparation cannot consume a workspace staged while it ran.
        if self.pending_storage == *prepared {
            self.pending_storage = None;
        }
    }

    pub(super) fn storage_changes_pending(&self, default_base: Option<&Path>) -> bool {
        let Some(connection) = &self.connection else {
            return false;
        };
        let Some(draft) = self
            .storage_draft
            .as_ref()
            .filter(|draft| draft.project_url.as_deref() == Some(connection.project.url()))
        else {
            return false;
        };
        let Some(storage) = &connection.storage else {
            return false;
        };
        if crate::overleaf::ProjectUrl::parse(&self.query)
            .as_ref()
            .ok()
            != Some(&connection.project)
        {
            return false;
        }
        let Ok(preference) = draft.preference() else {
            return true;
        };
        if preference.keep_local_copy != storage.persistent() {
            return true;
        }
        if !preference.keep_local_copy {
            return false;
        }
        let options = crate::overleaf::StorageOptions {
            keep_local_copy: true,
            directory: preference.directory.map(Into::into),
        };
        let Some(base) = default_base else {
            return true;
        };
        options
            .root_hint(&connection.project, base)
            .ok()
            .flatten()
            .as_deref()
            != Some(&connection.root)
    }

    pub(super) fn reset_storage_draft(&mut self, default_base: Option<&Path>) {
        let staged = self.pending_storage.take().is_some();
        if staged {
            self.storage_draft = None;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        if crate::overleaf::ProjectUrl::parse(&self.query)
            .as_ref()
            .ok()
            != Some(&connection.project)
        {
            return;
        }
        let Some(preference) = active_preference(connection, default_base) else {
            return;
        };
        self.storage_draft = Some(StorageDraft::new(connection.project.url(), &preference));
        if self.error.as_deref() == Some(STORAGE_CHANGE_OPEN_TABS) {
            self.error = None;
        }
    }
}

fn active_preference(
    connection: &Connection,
    default_base: Option<&Path>,
) -> Option<OverleafStoragePreference> {
    let storage = connection.storage.as_ref()?;
    let directory = if storage.persistent() {
        let base = storage.root().parent()?;
        (Some(base) != default_base).then(|| base.to_string_lossy().into_owned())
    } else {
        None
    };
    Some(OverleafStoragePreference {
        keep_local_copy: storage.persistent(),
        directory,
    })
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct StorageUiAction {
    pub(super) choose_folder: bool,
    pub(super) changed: bool,
    pub(super) apply: bool,
    pub(super) reset: bool,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    draft: &mut StorageDraft,
    default_base: Option<&Path>,
    busy: bool,
    pending: bool,
    staged: bool,
) -> StorageUiAction {
    let mut action = StorageUiAction::default();
    ui.add_enabled_ui(!busy, |ui| {
        // Reserve the tallest control before placing the first one: ordinary
        // horizontal rows cannot recenter earlier widgets after a taller button.
        let row_height = ui.spacing().interact_size.y.max(24.0);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), row_height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let keep = ui.checkbox(&mut draft.keep_local_copy, "Keep local copy");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("overleaf.keep_local_copy", &keep);
                action.changed |= keep.changed();
                let pending_labels: &[&str] = if pending {
                    &["Apply", "Reset"]
                } else if staged {
                    &["Reset"]
                } else {
                    &[]
                };
                let pending_width = pending_labels
                    .iter()
                    .map(|label| {
                        ui.painter()
                            .layout_no_wrap(
                                (*label).into(),
                                egui::TextStyle::Button.resolve(ui.style()),
                                ui.visuals().text_color(),
                            )
                            .size()
                            .x
                            + 2.0 * ui.spacing().button_padding.x
                            + ui.spacing().item_spacing.x
                    })
                    .sum::<f32>();
                if draft.keep_local_copy {
                    let button_size = row_height;
                    let gap = ui.spacing().item_spacing.x;
                    let width =
                        (ui.available_width() - button_size - gap - pending_width).max(40.0);
                    let hint = if default_base.is_some() {
                        "Application Support (default)"
                    } else {
                        "Default folder"
                    };
                    let directory = ui.add(
                        egui::TextEdit::singleline(&mut draft.directory)
                            .id_salt("overleaf-storage-directory")
                            .hint_text(hint)
                            .desired_width(width),
                    );
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("overleaf.directory", &directory);
                    action.changed |= directory.changed();
                    // Use the shared vector glyph without adding a hover popup.
                    let folder =
                        ui.add(egui::Button::new("").min_size(egui::Vec2::splat(button_size)));
                    folder.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            folder.enabled(),
                            "Choose local folder",
                        )
                    });
                    let color = ui.style().interact(&folder).fg_stroke.color;
                    crate::app::icons::paint_ui_icon(
                        ui.painter(),
                        folder.rect.shrink(5.0),
                        crate::app::icons::UiIcon::Folder,
                        color,
                    );
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("overleaf.choose_folder", &folder);
                    action.choose_folder = folder.clicked();
                } else {
                    ui.label(egui::RichText::new("Removed when project closes").weak());
                }
                if pending {
                    let apply = ui.button("Apply");
                    super::observe("apply_storage", &apply);
                    action.apply = apply.clicked();
                }
                if pending || staged {
                    let reset = ui.button("Reset");
                    super::observe("reset_storage", &reset);
                    action.reset = reset.clicked();
                }
            },
        );
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _},
    };

    const PROJECT: &str = "https://www.overleaf.com/project/0123456789abcdef01234567";

    struct Fixture {
        draft: StorageDraft,
        busy: bool,
        changed: bool,
        choose_folder: bool,
        pending: bool,
        staged: bool,
        apply: bool,
        reset: bool,
    }

    fn fixture() -> Fixture {
        Fixture {
            draft: StorageDraft::new(PROJECT, &OverleafStoragePreference::default()),
            busy: false,
            changed: false,
            choose_folder: false,
            pending: false,
            staged: false,
            apply: false,
            reset: false,
        }
    }

    fn render(ui: &mut egui::Ui, state: &mut Fixture) {
        let action = show(
            ui,
            &mut state.draft,
            Some(Path::new("/private/default/overleaf")),
            state.busy,
            state.pending,
            state.staged,
        );
        state.changed |= action.changed;
        state.choose_folder |= action.choose_folder;
        state.apply |= action.apply;
        state.reset |= action.reset;
    }

    #[test]
    fn draft_preserves_project_and_validates_folder_without_filesystem_access() {
        let mut draft = StorageDraft::new(PROJECT, &OverleafStoragePreference::default());
        assert_eq!(draft.project_url.as_deref(), Some(PROJECT));
        assert_eq!(
            draft.preference().unwrap(),
            OverleafStoragePreference::default()
        );
        draft.directory = "/workspace/unused/../overleaf".into();
        assert_eq!(
            draft.preference().unwrap().directory.as_deref(),
            Some("/workspace/overleaf")
        );
        draft.directory = "relative/folder".into();
        assert!(draft.preference().unwrap_err().contains("absolute"));
        draft.directory = "   ".into();
        assert_eq!(draft.preference().unwrap().directory, None);
    }

    #[test]
    fn pending_workspace_storage_is_editable_before_a_project_and_has_reset_without_apply() {
        let mut state = fixture();
        state.staged = true;
        state.draft = StorageDraft::unbound(&OverleafStoragePreference {
            keep_local_copy: true,
            directory: Some("/projects/new-workspace".into()),
        });
        let mut harness = Harness::builder()
            .with_size(egui::vec2(520.0, 100.0))
            .build_ui_state(render, state);
        harness.run();
        harness.get_by_label("Keep local copy");
        harness.get_by_label("Choose local folder").click();
        harness.run();
        assert!(harness.state().choose_folder);
        assert!(harness.query_by_label("Apply").is_none());
        let folder = harness.get_by_label("Choose local folder").rect();
        let reset = harness.get_by_label("Reset").rect();
        assert!(folder.right() <= reset.left());
        assert!(reset.right() <= 520.0);
        harness.get_by_label("Reset").click();
        harness.run();
        assert!(harness.state().reset);
        assert!(harness.state().draft.project_url.is_none());
    }

    #[test]
    fn pending_workspace_folder_survives_login_and_project_changes_until_adopted_or_reset() {
        let settings = crate::settings::AppSettings::default();
        let preference = OverleafStoragePreference {
            keep_local_copy: true,
            directory: Some("/projects/new-workspace".into()),
        };
        let mut state = State {
            pending_storage: Some(preference.clone()),
            ..State::default()
        };
        let first = crate::overleaf::ProjectUrl::parse(PROJECT).unwrap();
        let second = crate::overleaf::ProjectUrl::parse(
            "https://www.overleaf.com/project/ffffffffffffffffffffffff",
        )
        .unwrap();
        for project in [None, Some(&first), None, Some(&second), Some(&first)] {
            state.ensure_storage_draft(project, &settings).unwrap();
            let draft = state.storage_draft.as_ref().unwrap();
            assert_eq!(
                draft.project_url.as_deref(),
                project.map(|project| project.url())
            );
            assert_eq!(draft.preference().unwrap(), preference);
        }
        state.accept_pending_storage(&None);
        assert_eq!(state.pending_storage, Some(preference.clone()));
        let old = OverleafStoragePreference {
            directory: Some("/projects/older".into()),
            ..preference.clone()
        };
        state.accept_pending_storage(&Some(old));
        assert_eq!(state.pending_storage, Some(preference.clone()));
        state.accept_pending_storage(&Some(preference.clone()));
        assert!(state.pending_storage.is_none());
        state.pending_storage = Some(preference);
        state.reset_storage_draft(None);
        assert!(state.pending_storage.is_none());
        assert!(state.storage_draft.is_none());
    }

    #[test]
    fn temporary_mode_ignores_invalid_folder_without_losing_unfinished_draft() {
        let mut draft = StorageDraft::new(PROJECT, &OverleafStoragePreference::default());
        draft.directory = "unfinished/relative".into();
        draft.keep_local_copy = false;
        assert_eq!(
            draft.preference().unwrap(),
            OverleafStoragePreference {
                keep_local_copy: false,
                directory: None,
            }
        );
        assert_eq!(draft.directory, "unfinished/relative");
        draft.keep_local_copy = true;
        assert!(draft.preference().unwrap_err().contains("absolute"));
    }

    #[test]
    fn temporary_mode_hides_the_raw_folder_and_preserves_it_for_reenabling() {
        let mut state = fixture();
        state.draft.directory = "/private/local/overleaf".into();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1040.0, 100.0))
            .build_ui_state(render, state);
        harness.run();
        harness.get_by_label("Keep local copy").click();
        harness.run();
        assert!(!harness.state().draft.keep_local_copy);
        assert!(harness.state().changed);
        assert!(
            harness
                .query_by_role(egui::accesskit::Role::TextInput)
                .is_none()
        );
        assert!(harness.query_by_label("Choose local folder").is_none());
        harness.get_by_label("Removed when project closes");
        assert_eq!(harness.state().draft.directory, "/private/local/overleaf");
        harness.get_by_label("Keep local copy").click();
        harness.run();
        assert!(harness.state().draft.keep_local_copy);
        harness.get_by_role(egui::accesskit::Role::TextInput);
    }

    #[test]
    fn folder_field_accepts_typing_and_browse_requests_without_connecting() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1040.0, 100.0))
            .build_ui_state(render, fixture());
        harness.run();
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .click();
        harness.run();
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("/workspace/overleaf");
        harness.run();
        assert_eq!(harness.state().draft.directory, "/workspace/overleaf");
        assert!(harness.state().changed);
        harness.get_by_label("Choose local folder").click();
        harness.run();
        assert!(harness.state().choose_folder);
        assert_eq!(
            harness
                .state()
                .draft
                .preference()
                .unwrap()
                .directory
                .as_deref(),
            Some("/workspace/overleaf")
        );
    }

    #[test]
    fn busy_storage_controls_are_disabled_and_cannot_mutate_the_draft() {
        let mut state = fixture();
        state.busy = true;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1040.0, 100.0))
            .build_ui_state(render, state);
        harness.run();
        for label in ["Keep local copy", "Choose local folder"] {
            assert!(harness.get_by_label(label).accesskit_node().is_disabled());
            harness.get_by_label(label).click();
            harness.run();
        }
        assert!(
            harness
                .get_by_role(egui::accesskit::Role::TextInput)
                .accesskit_node()
                .is_disabled()
        );
        assert!(harness.state().draft.keep_local_copy);
        assert!(!harness.state().changed);
        assert!(!harness.state().choose_folder);
    }

    #[test]
    fn persistent_controls_fit_one_compact_row() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1040.0, 100.0))
            .build_ui_state(render, fixture());
        harness.run();
        let keep = harness.get_by_label("Keep local copy").rect();
        let directory = harness.get_by_role(egui::accesskit::Role::TextInput).rect();
        let folder = harness.get_by_label("Choose local folder").rect();
        assert!(keep.right() <= directory.left());
        assert!(directory.right() <= folder.left());
        assert!(folder.right() <= 1040.0);
        assert!(folder.height() <= 28.0);
        assert!(
            (keep.center().y - folder.center().y).abs() < 0.5,
            "checkbox {keep:?}, folder {folder:?}"
        );
        assert!(
            (directory.center().y - folder.center().y).abs() < 0.5,
            "directory {directory:?}, folder {folder:?}"
        );
    }
    #[test]
    fn pending_storage_actions_are_explicit_and_stay_in_the_compact_row() {
        for keep in [false, true] {
            let mut state = fixture();
            state.pending = true;
            state.draft.keep_local_copy = keep;
            let mut harness = Harness::builder()
                .with_size(egui::vec2(520.0, 100.0))
                .build_ui_state(render, state);
            harness.run();
            let keep = harness.get_by_label("Keep local copy").rect();
            let apply = harness.get_by_label("Apply").rect();
            let reset = harness.get_by_label("Reset").rect();
            assert!(apply.right() <= reset.left());
            assert!(reset.right() <= 520.0, "{reset:?}");
            assert!((keep.center().y - apply.center().y).abs() < 0.5);
            assert!((keep.center().y - reset.center().y).abs() < 0.5);
            harness.get_by_label("Apply").click();
            harness.run();
            assert!(harness.state().apply);
            assert!(!harness.state().reset);
            harness.get_by_label("Reset").click();
            harness.run();
            assert!(harness.state().reset);
            harness.state_mut().pending = false;
            harness.run();
            assert!(harness.query_by_label("Apply").is_none());
            assert!(harness.query_by_label("Reset").is_none());
        }
    }

    #[test]
    fn preparing_storage_disables_pending_apply_and_reset() {
        let mut state = fixture();
        state.pending = true;
        state.busy = true;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(520.0, 100.0))
            .build_ui_state(render, state);
        harness.run();
        for label in ["Apply", "Reset"] {
            assert!(harness.get_by_label(label).accesskit_node().is_disabled());
            harness.get_by_label(label).click();
            harness.run();
        }
        assert!(!harness.state().apply);
        assert!(!harness.state().reset);
    }
}
