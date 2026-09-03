use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use eframe::{Storage, egui};
use serde::{Deserialize, Serialize};

use crate::{builtin_themes, sublime_theme};

// Preserve the pre-rename key so existing installations keep their settings.
const STORAGE_KEY: &str = "mytypst.settings.v1";
pub(crate) const DEFAULT_HOVER_DELAY_MS: u64 = 300;
pub(crate) const DEFAULT_HOVER_FADE_MS: u64 = 90;
pub(crate) const MAX_RECENT_WORKSPACES: usize = 10;
pub(crate) const SYSTEM_THEME_ID: &str = "system";

/// The interface appearance selected by the user.
///
/// This is deliberately owned by tiptoptyp rather than inferred from egui's
/// persisted memory, so the Settings panel remains the single source of truth.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum InterfaceTheme {
    #[default]
    System,
    Light,
    Dark,
}

impl InterfaceTheme {
    pub(crate) const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => "Follow system",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub(crate) fn resolve(
        self,
        system_theme: Option<egui::Theme>,
        fallback: egui::Theme,
    ) -> egui::Theme {
        match self {
            Self::System => system_theme.unwrap_or(fallback),
            Self::Light => egui::Theme::Light,
            Self::Dark => egui::Theme::Dark,
        }
    }

    pub(crate) fn fallback_reason(self, system_theme: Option<egui::Theme>) -> Option<&'static str> {
        (self == Self::System && system_theme.is_none())
            .then_some("System appearance is unavailable; using the dark fallback")
    }
}

/// One color-scheme source for a particular interface appearance.
///
/// Keeping the source as an enum prevents a stale imported path from silently
/// overriding a built-in selection. `AppSettings` owns one choice for light
/// appearance and another for dark appearance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", content = "value", rename_all = "snake_case")]
pub(crate) enum ColorThemeChoice {
    Builtin(String),
    Sublime(String),
}

impl ColorThemeChoice {
    pub(crate) fn builtin(id: impl Into<String>) -> Self {
        Self::Builtin(id.into())
    }

    pub(crate) fn sublime(path: impl Into<String>) -> Self {
        Self::Sublime(path.into())
    }
}

/// Page colours are independent from the surrounding application chrome.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DocumentTheme {
    #[default]
    FollowInterface,
    Light,
    Dark,
}

impl DocumentTheme {
    pub(crate) const ALL: [Self; 3] = [Self::FollowInterface, Self::Light, Self::Dark];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FollowInterface => "Follow interface",
            Self::Light => "Light pages",
            Self::Dark => "Dark pages",
        }
    }

    pub(crate) fn resolve(self, interface_theme: egui::Theme) -> egui::Theme {
        match self {
            Self::FollowInterface => interface_theme,
            Self::Light => egui::Theme::Light,
            Self::Dark => egui::Theme::Dark,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PreviewPreference {
    #[default]
    Interactive,
    Native,
}

/// Mouse gesture used for an explicit source-to-preview jump.
///
/// Keeping this separate from automatic cursor synchronization makes the
/// interaction deliberate and gives trackpad users a gesture that does not
/// require a modifier key.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SourcePreviewTrigger {
    #[default]
    DoubleClick,
    ModifierClick,
    Disabled,
}

impl SourcePreviewTrigger {
    pub(crate) const ALL: [Self; 3] = [Self::DoubleClick, Self::ModifierClick, Self::Disabled];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::DoubleClick => "Double-click",
            Self::ModifierClick => "Command/Ctrl-click",
            Self::Disabled => "Disabled",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::DoubleClick => {
                "Double-click source text to reveal it in the interactive preview."
            }
            Self::ModifierClick => {
                "Command-click on macOS or Ctrl-click elsewhere to reveal source in the preview."
            }
            Self::Disabled => "Source-to-preview mouse navigation is disabled.",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ToolMode {
    #[default]
    Bundled,
    Custom,
}

impl ToolMode {
    pub(crate) const ALL: [Self; 2] = [Self::Bundled, Self::Custom];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Bundled => "Bundled",
            Self::Custom => "Custom path",
        }
    }
}

/// One executable choice. Keeping the last custom path while Bundled is
/// selected makes it possible to switch between the two without re-browsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ToolPreference {
    pub(crate) mode: ToolMode,
    pub(crate) custom_path: String,
}

impl Default for ToolPreference {
    fn default() -> Self {
        Self {
            mode: ToolMode::Bundled,
            custom_path: String::new(),
        }
    }
}

impl PreviewPreference {
    pub(crate) const ALL: [Self; 2] = [Self::Interactive, Self::Native];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Interactive => "Interactive (Tinymist)",
            Self::Native => "Rasterised PDF",
        }
    }
}

/// Persisted user choices. New fields must have defaults so older settings
/// files remain forwards-compatible as this panel grows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    pub(crate) interface_theme: InterfaceTheme,
    /// Color scheme used whenever the effective interface appearance is light.
    pub(crate) light_theme: ColorThemeChoice,
    /// Color scheme used whenever the effective interface appearance is dark.
    pub(crate) dark_theme: ColorThemeChoice,
    /// Applied before hue rotation to the complete semantic and syntax palette.
    pub(crate) theme_invert: bool,
    /// Whole-theme hue rotation, in degrees, applied after optional inversion.
    pub(crate) theme_hue_shift_degrees: i16,
    pub(crate) document_theme: DocumentTheme,
    pub(crate) preview_preference: PreviewPreference,
    pub(crate) line_wrap: bool,
    pub(crate) line_numbers: bool,
    pub(crate) source_preview_trigger: SourcePreviewTrigger,
    pub(crate) auto_save: bool,
    pub(crate) auto_save_delay_ms: u64,
    pub(crate) hover_delay_ms: u64,
    pub(crate) hover_fade_ms: u64,
    /// Most recently used canonical workspace roots, newest first.
    #[serde(default)]
    pub(crate) recent_workspaces: Vec<String>,
    /// Last successfully opened source document, keyed by canonical project root.
    pub(crate) last_opened_files: BTreeMap<String, String>,
    /// Designated Typst preview entry point, keyed by canonical project root.
    pub(crate) preview_files: BTreeMap<String, String>,
    pub(crate) typst: ToolPreference,
    pub(crate) tinymist: ToolPreference,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            interface_theme: InterfaceTheme::System,
            light_theme: ColorThemeChoice::builtin("tiptop-light"),
            dark_theme: ColorThemeChoice::builtin("tiptop-dark"),
            theme_invert: false,
            theme_hue_shift_degrees: 0,
            document_theme: DocumentTheme::FollowInterface,
            preview_preference: PreviewPreference::Interactive,
            line_wrap: true,
            line_numbers: true,
            source_preview_trigger: SourcePreviewTrigger::DoubleClick,
            auto_save: true,
            auto_save_delay_ms: 750,
            hover_delay_ms: DEFAULT_HOVER_DELAY_MS,
            hover_fade_ms: DEFAULT_HOVER_FADE_MS,
            recent_workspaces: Vec::new(),
            last_opened_files: BTreeMap::new(),
            preview_files: BTreeMap::new(),
            typst: ToolPreference::default(),
            tinymist: ToolPreference::default(),
        }
    }
}

impl AppSettings {
    pub(crate) fn color_theme(&self, appearance: egui::Theme) -> &ColorThemeChoice {
        match appearance {
            egui::Theme::Light => &self.light_theme,
            egui::Theme::Dark => &self.dark_theme,
        }
    }

    pub(crate) fn color_theme_mut(&mut self, appearance: egui::Theme) -> &mut ColorThemeChoice {
        match appearance {
            egui::Theme::Light => &mut self.light_theme,
            egui::Theme::Dark => &mut self.dark_theme,
        }
    }

    /// Move a successfully opened workspace to the front of the recent list.
    ///
    /// Callers pass a canonical or otherwise absolute path; keeping the helper
    /// lexical avoids filesystem access on the UI thread.
    pub(crate) fn remember_workspace(&mut self, workspace: &Path) {
        let workspace = workspace.to_string_lossy().into_owned();
        if workspace.is_empty() {
            return;
        }

        self.recent_workspaces
            .retain(|candidate| candidate != &workspace);
        self.recent_workspaces.insert(0, workspace);
        self.recent_workspaces.truncate(MAX_RECENT_WORKSPACES);
    }

    /// Return displayable workspace roots without mutating persisted history.
    /// Missing entries can become valid again when a removable volume returns.
    pub(crate) fn existing_recent_workspaces(&self) -> Vec<PathBuf> {
        self.recent_workspaces
            .iter()
            .map(PathBuf::from)
            .filter(|workspace| workspace.is_dir())
            .collect()
    }

    pub(crate) fn load(storage: Option<&dyn Storage>) -> Self {
        let Some(serialized) = storage.and_then(|storage| storage.get_string(STORAGE_KEY)) else {
            return Self::default();
        };
        let mut settings: Self = serde_json::from_str(&serialized).unwrap_or_default();
        let legacy = serde_json::from_str::<LegacyThemeSettings>(&serialized).unwrap_or_default();
        settings.migrate_legacy_theme(legacy);
        settings.normalize_builtin_theme_slots();
        settings
    }

    fn normalize_builtin_theme_slots(&mut self) {
        for (choice, dark_mode) in [(&mut self.light_theme, false), (&mut self.dark_theme, true)] {
            let valid = match choice {
                ColorThemeChoice::Builtin(id) => {
                    builtin_themes::find(id).is_some_and(|theme| theme.dark_mode == dark_mode)
                }
                ColorThemeChoice::Sublime(_) => true,
            };
            if !valid {
                *choice = ColorThemeChoice::builtin(builtin_themes::default_for_mode(dark_mode).id);
            }
        }
    }

    fn migrate_legacy_theme(&mut self, legacy: LegacyThemeSettings) {
        // A new-format file can omit either slot and receive its default. It
        // must never be mistaken for the old single-selection schema.
        if legacy.light_theme.is_some() || legacy.dark_theme.is_some() {
            return;
        }

        let fallback_dark =
            self.interface_theme.resolve(None, egui::Theme::Dark) == egui::Theme::Dark;
        let legacy_choice = legacy
            .sublime_theme_path
            .filter(|path| !path.is_empty())
            .map(|path| {
                let dark_mode = sublime_theme::import_path(Path::new(&path))
                    .map_or(fallback_dark, |theme| theme.dark_mode);
                (ColorThemeChoice::sublime(path), dark_mode)
            })
            .or_else(|| {
                let id = legacy.builtin_theme_id.filter(|id| !id.is_empty())?;
                if id == SYSTEM_THEME_ID {
                    return None;
                }
                let dark_mode =
                    builtin_themes::find(&id).map_or(fallback_dark, |theme| theme.dark_mode);
                Some((ColorThemeChoice::builtin(id), dark_mode))
            });

        let Some((choice, dark_mode)) = legacy_choice else {
            return;
        };
        *self.color_theme_mut(if dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        }) = choice;
        // The old source was fixed rather than paired. Preserve what the user
        // saw at migration time instead of letting the other slot become active
        // immediately because the OS currently has the opposite appearance.
        self.interface_theme = if dark_mode {
            InterfaceTheme::Dark
        } else {
            InterfaceTheme::Light
        };
    }

    pub(crate) fn save(&self, storage: &mut dyn Storage) {
        if let Ok(serialized) = serde_json::to_string(self) {
            storage.set_string(STORAGE_KEY, serialized);
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyThemeSettings {
    builtin_theme_id: Option<String>,
    sublime_theme_path: Option<String>,
    light_theme: Option<ColorThemeChoice>,
    dark_theme: Option<ColorThemeChoice>,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct MemoryStorage(HashMap<String, String>);

    impl Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }

        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }

        fn flush(&mut self) {}
    }

    #[test]
    fn defaults_follow_the_system_and_interface() {
        let settings = AppSettings::default();
        assert_eq!(settings.interface_theme, InterfaceTheme::System);
        assert_eq!(
            settings.light_theme,
            ColorThemeChoice::builtin("tiptop-light")
        );
        assert_eq!(
            settings.dark_theme,
            ColorThemeChoice::builtin("tiptop-dark")
        );
        assert!(!settings.theme_invert);
        assert_eq!(settings.theme_hue_shift_degrees, 0);
        assert_eq!(settings.document_theme, DocumentTheme::FollowInterface);
        assert_eq!(settings.preview_preference, PreviewPreference::Interactive);
        assert!(settings.line_wrap);
        assert!(settings.line_numbers);
        assert_eq!(
            settings.source_preview_trigger,
            SourcePreviewTrigger::DoubleClick
        );
        assert!(settings.auto_save);
        assert_eq!(settings.auto_save_delay_ms, 750);
        assert_eq!(settings.hover_delay_ms, DEFAULT_HOVER_DELAY_MS);
        assert_eq!(settings.hover_fade_ms, DEFAULT_HOVER_FADE_MS);
        assert!(settings.last_opened_files.is_empty());
        assert!(settings.preview_files.is_empty());
        assert!(settings.recent_workspaces.is_empty());
        assert_eq!(settings.typst.mode, ToolMode::Bundled);
        assert_eq!(settings.tinymist.mode, ToolMode::Bundled);
    }

    #[test]
    fn system_theme_resolves_without_oscillating_explicit_choices() {
        assert_eq!(
            InterfaceTheme::System.resolve(Some(egui::Theme::Light), egui::Theme::Dark),
            egui::Theme::Light
        );
        assert_eq!(
            InterfaceTheme::System.resolve(Some(egui::Theme::Dark), egui::Theme::Dark),
            egui::Theme::Dark
        );
        assert_eq!(
            InterfaceTheme::Dark.resolve(Some(egui::Theme::Light), egui::Theme::Light),
            egui::Theme::Dark
        );
        assert_eq!(
            InterfaceTheme::Light.resolve(Some(egui::Theme::Dark), egui::Theme::Dark),
            egui::Theme::Light
        );
    }

    #[test]
    fn unknown_system_theme_has_an_explicit_fallback_status() {
        assert_eq!(
            InterfaceTheme::System.resolve(None, egui::Theme::Dark),
            egui::Theme::Dark
        );
        assert!(InterfaceTheme::System.fallback_reason(None).is_some());
        assert!(
            InterfaceTheme::System
                .fallback_reason(Some(egui::Theme::Light))
                .is_none()
        );
        assert!(InterfaceTheme::Dark.fallback_reason(None).is_none());
    }

    #[test]
    fn document_theme_can_follow_or_override_the_interface() {
        assert_eq!(
            DocumentTheme::FollowInterface.resolve(egui::Theme::Light),
            egui::Theme::Light
        );
        assert_eq!(
            DocumentTheme::Dark.resolve(egui::Theme::Light),
            egui::Theme::Dark
        );
        assert_eq!(
            DocumentTheme::Light.resolve(egui::Theme::Dark),
            egui::Theme::Light
        );
    }

    #[test]
    fn persisted_settings_round_trip_and_missing_fields_use_defaults() {
        let expected = AppSettings {
            interface_theme: InterfaceTheme::Light,
            light_theme: ColorThemeChoice::builtin("catppuccin-latte"),
            dark_theme: ColorThemeChoice::sublime("/themes/Example.sublime-color-scheme"),
            theme_invert: true,
            theme_hue_shift_degrees: -45,
            document_theme: DocumentTheme::Dark,
            preview_preference: PreviewPreference::Native,
            line_wrap: false,
            line_numbers: false,
            source_preview_trigger: SourcePreviewTrigger::ModifierClick,
            auto_save: false,
            auto_save_delay_ms: 1_500,
            hover_delay_ms: 450,
            hover_fade_ms: 120,
            last_opened_files: BTreeMap::from([(
                "/workspace".to_owned(),
                "/workspace/main.typ".to_owned(),
            )]),
            preview_files: BTreeMap::from([(
                "/workspace".to_owned(),
                "/workspace/main.typ".to_owned(),
            )]),
            recent_workspaces: vec!["/workspace".to_owned()],
            typst: ToolPreference {
                mode: ToolMode::Custom,
                custom_path: "/opt/typst".to_owned(),
            },
            tinymist: ToolPreference {
                mode: ToolMode::Custom,
                custom_path: "/opt/tinymist".to_owned(),
            },
        };
        let mut storage = MemoryStorage::default();
        expected.save(&mut storage);
        assert_eq!(AppSettings::load(Some(&storage)), expected);

        storage.set_string(STORAGE_KEY, r#"{"interface_theme":"Dark"}"#.to_owned());
        let partial = AppSettings::load(Some(&storage));
        assert_eq!(partial.interface_theme, InterfaceTheme::Dark);
        assert_eq!(
            partial.light_theme,
            ColorThemeChoice::builtin("tiptop-light")
        );
        assert_eq!(partial.dark_theme, ColorThemeChoice::builtin("tiptop-dark"));
        assert!(!partial.theme_invert);
        assert_eq!(partial.theme_hue_shift_degrees, 0);
        assert_eq!(partial.document_theme, DocumentTheme::FollowInterface);
        assert_eq!(partial.preview_preference, PreviewPreference::Interactive);
        assert!(partial.line_wrap);
        assert!(partial.line_numbers);
        assert_eq!(
            partial.source_preview_trigger,
            SourcePreviewTrigger::DoubleClick
        );
        assert!(partial.auto_save);
        assert_eq!(partial.auto_save_delay_ms, 750);
        assert_eq!(partial.hover_delay_ms, DEFAULT_HOVER_DELAY_MS);
        assert_eq!(partial.hover_fade_ms, DEFAULT_HOVER_FADE_MS);
        assert!(partial.last_opened_files.is_empty());
        assert!(partial.preview_files.is_empty());
        assert!(partial.recent_workspaces.is_empty());
        assert_eq!(partial.typst, ToolPreference::default());
        assert_eq!(partial.tinymist, ToolPreference::default());
    }

    #[test]
    fn old_single_builtin_theme_migrates_to_its_matching_slot() {
        let mut storage = MemoryStorage::default();
        storage.set_string(
            STORAGE_KEY,
            r#"{"interface_theme":"System","builtin_theme_id":"catppuccin-latte"}"#.to_owned(),
        );

        let settings = AppSettings::load(Some(&storage));
        assert_eq!(settings.interface_theme, InterfaceTheme::Light);
        assert_eq!(
            settings.light_theme,
            ColorThemeChoice::builtin("catppuccin-latte")
        );
        assert_eq!(
            settings.dark_theme,
            ColorThemeChoice::builtin("tiptop-dark")
        );
    }

    #[test]
    fn old_imported_theme_migrates_using_its_inferred_appearance() {
        let temp = tempfile::tempdir().expect("create temporary theme directory");
        let path = temp.path().join("Legacy.sublime-color-scheme");
        std::fs::write(
            &path,
            r##"{
                "name": "Legacy Dark",
                "globals": {
                    "background": "#20242c",
                    "foreground": "#e8ecf2"
                }
            }"##,
        )
        .expect("write legacy imported theme");
        let serialized = serde_json::json!({
            "interface_theme": "System",
            "builtin_theme_id": "paper-light",
            "sublime_theme_path": path.to_string_lossy(),
        })
        .to_string();
        let mut storage = MemoryStorage::default();
        storage.set_string(STORAGE_KEY, serialized);

        let settings = AppSettings::load(Some(&storage));
        assert_eq!(settings.interface_theme, InterfaceTheme::Dark);
        assert_eq!(
            settings.light_theme,
            ColorThemeChoice::builtin("tiptop-light")
        );
        assert_eq!(
            settings.dark_theme,
            ColorThemeChoice::sublime(path.to_string_lossy())
        );
    }

    #[test]
    fn new_theme_slots_are_not_reinterpreted_as_legacy_settings() {
        let mut settings = AppSettings {
            interface_theme: InterfaceTheme::System,
            light_theme: ColorThemeChoice::builtin("paper-light"),
            dark_theme: ColorThemeChoice::builtin("catppuccin-mocha"),
            ..AppSettings::default()
        };
        let mut storage = MemoryStorage::default();
        settings.save(&mut storage);
        settings.interface_theme = InterfaceTheme::System;

        assert_eq!(AppSettings::load(Some(&storage)), settings);
    }

    #[test]
    fn corrupt_or_cross_mode_builtin_slots_use_their_matching_defaults() {
        let mut storage = MemoryStorage::default();
        storage.set_string(
            STORAGE_KEY,
            serde_json::json!({
                "interface_theme": "System",
                "light_theme": { "source": "builtin", "value": "catppuccin-mocha" },
                "dark_theme": { "source": "builtin", "value": "missing-theme" }
            })
            .to_string(),
        );

        let settings = AppSettings::load(Some(&storage));
        assert_eq!(
            settings.light_theme,
            ColorThemeChoice::builtin("tiptop-light")
        );
        assert_eq!(
            settings.dark_theme,
            ColorThemeChoice::builtin("tiptop-dark")
        );
    }

    #[test]
    fn recording_recent_workspaces_is_mru_deduplicated_and_bounded() {
        let mut settings = AppSettings::default();
        for index in 0..12 {
            settings.remember_workspace(Path::new(&format!("/workspace/{index}")));
        }

        assert_eq!(settings.recent_workspaces.len(), MAX_RECENT_WORKSPACES);
        assert_eq!(settings.recent_workspaces[0], "/workspace/11");
        assert_eq!(settings.recent_workspaces[9], "/workspace/2");

        settings.remember_workspace(Path::new("/workspace/7"));
        assert_eq!(settings.recent_workspaces.len(), MAX_RECENT_WORKSPACES);
        assert_eq!(settings.recent_workspaces[0], "/workspace/7");
        assert_eq!(
            settings
                .recent_workspaces
                .iter()
                .filter(|path| path.as_str() == "/workspace/7")
                .count(),
            1
        );
    }

    #[test]
    fn existing_recent_workspaces_only_returns_directories_in_mru_order() {
        let temp = tempfile::tempdir().expect("create temporary workspace root");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let file = temp.path().join("not-a-folder.typ");
        std::fs::create_dir(&first).expect("create first workspace");
        std::fs::create_dir(&second).expect("create second workspace");
        std::fs::write(&file, "= document").expect("create ordinary file");

        let settings = AppSettings {
            recent_workspaces: vec![
                second.to_string_lossy().into_owned(),
                file.to_string_lossy().into_owned(),
                first.to_string_lossy().into_owned(),
                temp.path().join("missing").to_string_lossy().into_owned(),
            ],
            ..AppSettings::default()
        };

        assert_eq!(settings.existing_recent_workspaces(), vec![second, first]);
    }

    #[test]
    fn corrupt_persisted_settings_fail_closed_to_defaults() {
        let mut storage = MemoryStorage::default();
        storage.set_string(STORAGE_KEY, "not json".to_owned());
        assert_eq!(AppSettings::load(Some(&storage)), AppSettings::default());
    }
}
