use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use eframe::{Storage, egui};
use serde::{Deserialize, Serialize};

use crate::{
    builtin_themes,
    shortcuts::{ShortcutBindings, ShortcutOverrides},
    syntax_theme::TypstOverrideThemes,
};

const STORAGE_KEY: &str = "tiptoptyp.settings.v1";
pub(crate) const DEFAULT_HOVER_DELAY_MS: u64 = 300;
pub(crate) const DEFAULT_HOVER_FADE_MS: u64 = 90;
pub(crate) const MAX_RECENT_WORKSPACES: usize = 20;
pub(crate) const SYSTEM_THEME_ID: &str = "system";
pub(crate) const DEFAULT_UI_SCALE_PERCENT: u16 = 100;
pub(crate) const DEFAULT_UI_FONT_WEIGHT: u16 = 400;

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
            #[cfg(target_os = "macos")]
            Self::ModifierClick => "Command-click",
            #[cfg(not(target_os = "macos"))]
            Self::ModifierClick => "Control-click",
            Self::Disabled => "Disabled",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::DoubleClick => {
                "Double-click source text to reveal it in the interactive preview."
            }
            #[cfg(target_os = "macos")]
            Self::ModifierClick => "Command-click source text to reveal it in the preview.",
            #[cfg(not(target_os = "macos"))]
            Self::ModifierClick => "Control-click source text to reveal it in the preview.",
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

/// Persisted user choices for the current settings schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default = "default_true")]
    pub(crate) sticky_context_rows: bool,
    pub(crate) source_preview_trigger: SourcePreviewTrigger,
    pub(crate) auto_save: bool,
    pub(crate) auto_save_delay_ms: u64,
    pub(crate) hover_delay_ms: u64,
    pub(crate) hover_fade_ms: u64,
    /// Scale applied to the application chrome and editor UI.
    pub(crate) ui_scale_percent: u16,
    /// Use the editor's monospace family for interface text as well.
    pub(crate) ui_font_monospace: bool,
    /// A face path used to identify the selected system or workspace UI family.
    pub(crate) ui_font_path: Option<String>,
    /// OpenType family selected from `ui_font_path`. Keeping the family name
    /// lets collections and separately installed weight faces act as one font.
    pub(crate) ui_font_family: Option<String>,
    /// Face used as the persisted fallback until the font catalog is ready.
    pub(crate) ui_font_face_index: u32,
    /// OpenType weight used by proportional, monospace, and custom UI fonts.
    pub(crate) ui_font_weight: u16,
    /// Optional system or workspace family used by the source editor.
    pub(crate) code_font_path: Option<String>,
    pub(crate) code_font_family: Option<String>,
    pub(crate) code_font_face_index: u32,
    /// Base OpenType weight used by ordinary editor text. Syntax roles retain
    /// their relative emphasis around this value.
    pub(crate) code_font_weight: u16,
    /// Keep the in-window File/Edit/View controls visible beside the document
    /// title. Native macOS menus remain available when this is disabled.
    pub(crate) titlebar_menus: bool,
    /// User changes from the current platform's built-in command bindings.
    /// Action IDs and chords are normalized after deserialization.
    #[serde(default, skip_serializing_if = "ShortcutOverrides::is_empty")]
    pub(crate) shortcut_overrides: ShortcutOverrides,
    /// Optional Typst-only style layers for each interface appearance.
    pub(crate) typst_overrides: TypstOverrideThemes,
    /// Most recently used canonical workspace roots, newest first.
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
            sticky_context_rows: true,
            source_preview_trigger: SourcePreviewTrigger::DoubleClick,
            auto_save: true,
            auto_save_delay_ms: 750,
            hover_delay_ms: DEFAULT_HOVER_DELAY_MS,
            hover_fade_ms: DEFAULT_HOVER_FADE_MS,
            ui_scale_percent: DEFAULT_UI_SCALE_PERCENT,
            ui_font_monospace: false,
            ui_font_path: None,
            ui_font_family: None,
            ui_font_face_index: 0,
            ui_font_weight: DEFAULT_UI_FONT_WEIGHT,
            code_font_path: None,
            code_font_family: None,
            code_font_face_index: 0,
            code_font_weight: DEFAULT_UI_FONT_WEIGHT,
            titlebar_menus: true,
            shortcut_overrides: ShortcutOverrides::default(),
            typst_overrides: TypstOverrideThemes::default(),
            recent_workspaces: Vec::new(),
            last_opened_files: BTreeMap::new(),
            preview_files: BTreeMap::new(),
            typst: ToolPreference::default(),
            tinymist: ToolPreference::default(),
        }
    }
}

const fn default_true() -> bool {
    true
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
    /// Canonicalize the root once at the point it enters MRU history so path
    /// aliases cannot create duplicate workspace entries.
    pub(crate) fn remember_workspace(&mut self, workspace: &Path) {
        let workspace = normalize_workspace_root(workspace)
            .to_string_lossy()
            .into_owned();
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
        let mut roots = Vec::new();
        for workspace in &self.recent_workspaces {
            let workspace = PathBuf::from(workspace);
            if workspace.is_dir() && !roots.contains(&workspace) {
                roots.push(workspace);
            }
        }
        roots
    }

    pub(crate) fn forget_workspace(&mut self, workspace: &Path) {
        let workspace = normalize_workspace_root(workspace)
            .to_string_lossy()
            .into_owned();
        self.recent_workspaces.retain(|candidate| {
            normalize_workspace_root(Path::new(candidate)).to_string_lossy() != workspace
        });
    }

    pub(crate) fn load(storage: Option<&dyn Storage>) -> Self {
        let Some(serialized) = storage.and_then(|storage| storage.get_string(STORAGE_KEY)) else {
            return Self::default();
        };
        let mut settings: Self = serde_json::from_str(&serialized).unwrap_or_default();
        settings.ui_font_weight = settings.ui_font_weight.clamp(1, 1_000);
        settings.code_font_weight = settings.code_font_weight.clamp(1, 1_000);
        settings.normalize_builtin_theme_slots();
        settings.normalize_workspace_history();
        settings.shortcut_overrides.normalize();
        settings
    }

    fn normalize_workspace_history(&mut self) {
        let mut recent = Vec::new();
        for workspace in std::mem::take(&mut self.recent_workspaces) {
            let normalized = normalize_workspace_root(Path::new(&workspace))
                .to_string_lossy()
                .into_owned();
            if !normalized.is_empty() && !recent.contains(&normalized) {
                recent.push(normalized);
            }
        }
        recent.truncate(MAX_RECENT_WORKSPACES);
        self.recent_workspaces = recent;

        self.last_opened_files =
            normalize_workspace_map(std::mem::take(&mut self.last_opened_files));
        self.preview_files = normalize_workspace_map(std::mem::take(&mut self.preview_files));
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

    pub(crate) fn save(&self, storage: &mut dyn Storage) {
        let mut normalized = self.clone();
        normalized.normalize_workspace_history();
        normalized.shortcut_overrides.normalize();
        if let Ok(serialized) = serde_json::to_string(&normalized) {
            storage.set_string(STORAGE_KEY, serialized);
        }
    }

    pub(crate) fn effective_shortcuts(&self) -> ShortcutBindings {
        ShortcutBindings::current(&self.shortcut_overrides)
    }
}

/// Canonicalize workspace roots when possible and otherwise normalize their
/// lexical form without touching the filesystem. The fallback keeps startup
/// useful for roots that have temporarily disappeared.
pub(crate) fn normalize_workspace_root(path: &Path) -> PathBuf {
    let absolute = path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .map(|directory| directory.join(path))
                .unwrap_or_else(|_| path.to_owned())
        }
    });
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    normalized.components().next_back(),
                    Some(Component::RootDir | Component::Prefix(_))
                ) {
                    normalized.pop();
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn normalize_workspace_map(map: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut normalized = BTreeMap::new();
    for (workspace, file) in map {
        let workspace = normalize_workspace_root(Path::new(&workspace))
            .to_string_lossy()
            .into_owned();
        normalized.entry(workspace).or_insert(file);
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{
        shortcuts::{ShortcutAction, ShortcutChord, ShortcutPlatform},
        sublime_theme::Rgba,
        syntax_theme::TypstSyntaxRole,
        theme,
    };

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
        assert_eq!(settings.typst_overrides, TypstOverrideThemes::default());
        assert!(settings.shortcut_overrides.is_empty());
        assert!(settings.effective_shortcuts().conflicts().is_empty());
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
    fn persisted_settings_round_trip_and_partial_schemas_are_rejected() {
        let mut typst_overrides = TypstOverrideThemes::default();
        let light_function = typst_overrides
            .for_dark_mut(false)
            .get_mut_or_default(TypstSyntaxRole::Function);
        light_function.foreground = Some(Rgba::rgb(24, 80, 196));
        light_function.weight = Some(theme::FONT_WEIGHT_BOLD);
        let dark_comment = typst_overrides
            .for_dark_mut(true)
            .get_mut_or_default(TypstSyntaxRole::Comment);
        dark_comment.background = Some(Rgba::from_rgba(12, 18, 28, 180));
        dark_comment.italic = Some(false);
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
            sticky_context_rows: false,
            source_preview_trigger: SourcePreviewTrigger::ModifierClick,
            auto_save: false,
            auto_save_delay_ms: 1_500,
            hover_delay_ms: 450,
            hover_fade_ms: 120,
            ui_scale_percent: 115,
            ui_font_monospace: true,
            ui_font_path: Some("/fonts/Example.ttf".to_owned()),
            ui_font_family: Some("Example Sans".to_owned()),
            ui_font_face_index: 2,
            ui_font_weight: 550,
            code_font_path: Some("/fonts/ExampleCode.ttf".to_owned()),
            code_font_family: Some("Example Code".to_owned()),
            code_font_face_index: 1,
            code_font_weight: 450,
            titlebar_menus: false,
            shortcut_overrides: ShortcutOverrides::default(),
            typst_overrides,
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
        assert_eq!(AppSettings::load(Some(&storage)), AppSettings::default());
    }

    #[test]
    fn persisted_font_weights_are_normalized() {
        let mut storage = MemoryStorage::default();
        let settings = AppSettings {
            ui_font_weight: 0,
            code_font_weight: 5_000,
            ..AppSettings::default()
        };
        storage.set_string(
            STORAGE_KEY,
            serde_json::to_string(&settings).expect("settings serialize"),
        );

        let settings = AppSettings::load(Some(&storage));
        assert_eq!(settings.ui_font_weight, 1);
        assert_eq!(settings.code_font_weight, 1_000);
    }

    #[test]
    fn shortcut_overrides_round_trip_and_resolve_from_settings() {
        let mut settings = AppSettings::default();
        settings.shortcut_overrides.set(
            ShortcutAction::Save,
            Some(ShortcutChord::parse("Primary+Shift+K").unwrap()),
        );
        settings
            .shortcut_overrides
            .set(ShortcutAction::ExportPdf, None);
        let mut storage = MemoryStorage::default();
        settings.save(&mut storage);

        let restored = AppSettings::load(Some(&storage));
        assert_eq!(restored, settings);
        let shortcuts = restored.effective_shortcuts();
        assert_eq!(
            shortcuts
                .binding(ShortcutAction::Save)
                .unwrap()
                .config_string(),
            "Primary+Shift+K"
        );
        assert_eq!(shortcuts.binding(ShortcutAction::ExportPdf), None);
    }

    #[test]
    fn loading_settings_drops_only_invalid_shortcut_entries() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        value["shortcut_overrides"] = serde_json::json!({
            "file.save": "cmd + shift + k",
            "edit.copy": null,
            "edit.find": "Primary+NoSuchKey",
            "future.action": "Primary+Q",
        });
        let mut storage = MemoryStorage::default();
        storage.set_string(STORAGE_KEY, serde_json::to_string(&value).unwrap());

        let settings = AppSettings::load(Some(&storage));
        let shortcuts = settings.effective_shortcuts();
        assert_eq!(
            shortcuts
                .binding(ShortcutAction::Save)
                .unwrap()
                .config_string(),
            "Primary+Shift+K"
        );
        assert_eq!(shortcuts.binding(ShortcutAction::Copy), None);
        assert_eq!(
            shortcuts.binding(ShortcutAction::Find),
            crate::shortcuts::ShortcutBindings::defaults(ShortcutPlatform::current())
                .binding(ShortcutAction::Find)
        );

        let normalized = serde_json::to_value(&settings.shortcut_overrides).unwrap();
        assert_eq!(
            normalized,
            serde_json::json!({
                "edit.copy": null,
                "file.save": "Primary+Shift+K",
            })
        );
    }

    #[test]
    fn theme_slots_round_trip_without_reinterpretation() {
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
        let settings = AppSettings {
            light_theme: ColorThemeChoice::builtin("catppuccin-mocha"),
            dark_theme: ColorThemeChoice::builtin("missing-theme"),
            ..AppSettings::default()
        };
        storage.set_string(
            STORAGE_KEY,
            serde_json::to_string(&settings).expect("settings serialize"),
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
        for index in 0..25 {
            settings.remember_workspace(Path::new(&format!("/workspace/{index}")));
        }

        assert_eq!(settings.recent_workspaces.len(), MAX_RECENT_WORKSPACES);
        assert_eq!(settings.recent_workspaces[0], "/workspace/24");
        assert_eq!(settings.recent_workspaces[19], "/workspace/5");

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

        settings.forget_workspace(Path::new("/workspace/7/../7"));
        assert!(
            !settings
                .recent_workspaces
                .iter()
                .any(|path| path == "/workspace/7")
        );
    }

    #[test]
    fn saving_normalizes_deduplicates_and_bounds_workspace_history() {
        let mut settings = AppSettings {
            recent_workspaces: (0..25).map(|index| format!("/workspace/{index}")).collect(),
            ..AppSettings::default()
        };
        settings
            .recent_workspaces
            .insert(1, "/workspace/0/../0".to_owned());
        let mut storage = MemoryStorage::default();

        settings.save(&mut storage);
        let restored = AppSettings::load(Some(&storage));

        assert_eq!(restored.recent_workspaces.len(), MAX_RECENT_WORKSPACES);
        assert_eq!(restored.recent_workspaces[0], "/workspace/0");
        assert_eq!(
            restored
                .recent_workspaces
                .iter()
                .filter(|path| path.as_str() == "/workspace/0")
                .count(),
            1
        );
    }

    #[test]
    fn workspace_aliases_are_normalized_before_entering_history() {
        let temp = tempfile::tempdir().expect("create temporary workspace root");
        let root = temp.path().join("project");
        let nested = root.join("chapters");
        std::fs::create_dir_all(&nested).expect("create project folders");

        let mut settings = AppSettings::default();
        settings.remember_workspace(&root);
        settings.remember_workspace(&nested.join(".."));

        assert_eq!(settings.recent_workspaces.len(), 1);
        assert_eq!(
            PathBuf::from(&settings.recent_workspaces[0]),
            root.canonicalize().unwrap()
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
