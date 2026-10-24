//! Compact storage choices; connecting and folder dialogs remain controller work.
use crate::settings::OverleafStoragePreference;
use eframe::egui;
use std::path::Path;

#[derive(Clone, Debug)]
pub(super) struct StorageDraft {
    pub(super) project_url: String,
    pub(super) keep_local_copy: bool,
    pub(super) directory: String,
}

impl StorageDraft {
    pub(super) fn new(url: &str, preference: &OverleafStoragePreference) -> Self {
        Self {
            project_url: url.into(),
            keep_local_copy: preference.keep_local_copy,
            directory: preference.directory.clone().unwrap_or_default(),
        }
    }

    pub(super) fn preference(&self) -> Result<OverleafStoragePreference, String> {
        let preference = OverleafStoragePreference {
            keep_local_copy: self.keep_local_copy,
            directory: (!self.directory.trim().is_empty()).then(|| self.directory.clone()),
        };
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

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct StorageUiAction {
    pub(super) choose_folder: bool,
    pub(super) changed: bool,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    draft: &mut StorageDraft,
    default_base: Option<&Path>,
    busy: bool,
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
                if draft.keep_local_copy {
                    let button_size = row_height;
                    let gap = ui.spacing().item_spacing.x;
                    let width = (ui.available_width() - button_size - gap).max(40.0);
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
    }

    fn fixture() -> Fixture {
        Fixture {
            draft: StorageDraft::new(PROJECT, &OverleafStoragePreference::default()),
            busy: false,
            changed: false,
            choose_folder: false,
        }
    }

    fn render(ui: &mut egui::Ui, state: &mut Fixture) {
        let action = show(
            ui,
            &mut state.draft,
            Some(Path::new("/private/default/overleaf")),
            state.busy,
        );
        state.changed |= action.changed;
        state.choose_folder |= action.choose_folder;
    }

    #[test]
    fn draft_preserves_project_and_validates_folder_without_filesystem_access() {
        let mut draft = StorageDraft::new(PROJECT, &OverleafStoragePreference::default());
        assert_eq!(draft.project_url, PROJECT);
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
}
