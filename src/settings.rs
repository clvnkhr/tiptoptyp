use std::collections::BTreeMap;

use eframe::{Storage, egui};
use serde::{Deserialize, Serialize};

// Preserve the pre-rename key so existing installations keep their settings.
const STORAGE_KEY: &str = "mytypst.settings.v1";

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
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub(crate) fn egui_preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Light => egui::ThemePreference::Light,
            Self::Dark => egui::ThemePreference::Dark,
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
            Self::Native => "Native watched PDF",
        }
    }
}

/// Persisted user choices. New fields must have defaults so older settings
/// files remain forwards-compatible as this panel grows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    pub(crate) interface_theme: InterfaceTheme,
    pub(crate) document_theme: DocumentTheme,
    pub(crate) preview_preference: PreviewPreference,
    pub(crate) line_wrap: bool,
    pub(crate) line_numbers: bool,
    pub(crate) source_preview_trigger: SourcePreviewTrigger,
    pub(crate) auto_save: bool,
    pub(crate) auto_save_delay_ms: u64,
    /// Last successfully opened source document, keyed by canonical project root.
    pub(crate) last_opened_files: BTreeMap<String, String>,
    pub(crate) typst: ToolPreference,
    pub(crate) tinymist: ToolPreference,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            interface_theme: InterfaceTheme::System,
            document_theme: DocumentTheme::FollowInterface,
            preview_preference: PreviewPreference::Interactive,
            line_wrap: true,
            line_numbers: true,
            source_preview_trigger: SourcePreviewTrigger::DoubleClick,
            auto_save: true,
            auto_save_delay_ms: 750,
            last_opened_files: BTreeMap::new(),
            typst: ToolPreference::default(),
            tinymist: ToolPreference::default(),
        }
    }
}

impl AppSettings {
    pub(crate) fn load(storage: Option<&dyn Storage>) -> Self {
        storage
            .and_then(|storage| storage.get_string(STORAGE_KEY))
            .and_then(|serialized| serde_json::from_str(&serialized).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save(&self, storage: &mut dyn Storage) {
        if let Ok(serialized) = serde_json::to_string(self) {
            storage.set_string(STORAGE_KEY, serialized);
        }
    }
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
        assert!(settings.last_opened_files.is_empty());
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
            document_theme: DocumentTheme::Dark,
            preview_preference: PreviewPreference::Native,
            line_wrap: false,
            line_numbers: false,
            source_preview_trigger: SourcePreviewTrigger::ModifierClick,
            auto_save: false,
            auto_save_delay_ms: 1_500,
            last_opened_files: BTreeMap::from([(
                "/workspace".to_owned(),
                "/workspace/main.typ".to_owned(),
            )]),
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
        assert!(partial.last_opened_files.is_empty());
        assert_eq!(partial.typst, ToolPreference::default());
        assert_eq!(partial.tinymist, ToolPreference::default());
    }

    #[test]
    fn corrupt_persisted_settings_fail_closed_to_defaults() {
        let mut storage = MemoryStorage::default();
        storage.set_string(STORAGE_KEY, "not json".to_owned());
        assert_eq!(AppSettings::load(Some(&storage)), AppSettings::default());
    }
}
