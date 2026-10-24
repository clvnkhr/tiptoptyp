use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use eframe::{Storage, egui};
use serde::{Deserialize, Serialize};

use crate::{
    builtin_themes,
    explorer::ExplorerOrder,
    shortcuts::{ShortcutBindings, ShortcutOverrides},
    syntax_theme::TypstOverrideThemes,
};

const STORAGE_KEY: &str = "tiptoptyp.settings.v1";
const REJECTED_STORAGE_KEY: &str = "tiptoptyp.settings.rejected";

#[derive(Debug)]
pub(crate) struct SettingsLoadError {
    message: String,
    rejected: String,
}

impl std::fmt::Display for SettingsLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Saved settings could not be read: {}. Using defaults; the rejected settings will be retained in {REJECTED_STORAGE_KEY} when preferences are saved",
            self.message
        )
    }
}
fn default_liquid_threads() -> u8 {
    4
}

pub(crate) const DEFAULT_HOVER_DELAY_MS: u64 = 300;
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
    Pdfium,
    Interactive,
    #[default]
    InteractivePartial,
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
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RevealStyle {
    #[default]
    Rings,
    Pulse,
    Spotlight,
}
impl RevealStyle {
    pub(crate) const ALL: [Self; 3] = [Self::Rings, Self::Pulse, Self::Spotlight];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Rings => "Rings",
            Self::Pulse => "Pulse",
            Self::Spotlight => "Spotlight",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RevealAnimation {
    pub(crate) enabled: bool,
    pub(crate) style: RevealStyle,
    pub(crate) duration_ms: u16,
    pub(crate) size_percent: u16,
}
impl Default for RevealAnimation {
    fn default() -> Self {
        Self {
            enabled: true,
            style: RevealStyle::Rings,
            duration_ms: 900,
            size_percent: 100,
        }
    }
}
impl RevealAnimation {
    pub(crate) fn validate(self) -> Result<(), String> {
        if !(250..=2000).contains(&self.duration_ms) {
            return Err("duration_ms: choose a duration between 250 and 2000 milliseconds".into());
        }
        if !(50..=200).contains(&self.size_percent) {
            return Err("size_percent: choose a size between 50 and 200 percent".into());
        }
        Ok(())
    }
    pub(crate) fn normalized(self) -> Self {
        Self {
            duration_ms: self.duration_ms.clamp(250, 2000),
            size_percent: self.size_percent.clamp(50, 200),
            ..self
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ParticleDensity {
    Low,
    #[default]
    Medium,
    High,
}
impl ParticleDensity {
    pub(crate) const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ParticleStyle {
    #[default]
    Explosion,
    Shotgun,
    Flamethrower,
    Railgun,
    Plasma,
}
impl ParticleStyle {
    pub(crate) const ALL: [Self; 5] = [
        Self::Explosion,
        Self::Shotgun,
        Self::Flamethrower,
        Self::Railgun,
        Self::Plasma,
    ];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Explosion => "Explosion",
            Self::Shotgun => "Shotgun",
            Self::Flamethrower => "Flamethrower",
            Self::Railgun => "Railgun",
            Self::Plasma => "Plasma",
        }
    }
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum EditorAction {
    #[default]
    Typing,
    Newline,
    Indent,
    Delete,
    Comment,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
}
impl EditorAction {
    pub(crate) const ALL: [Self; 10] = [
        Self::Typing,
        Self::Newline,
        Self::Indent,
        Self::Delete,
        Self::Comment,
        Self::Undo,
        Self::Redo,
        Self::Copy,
        Self::Cut,
        Self::Paste,
    ];
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Typing => "Typing",
            Self::Newline => "Newline",
            Self::Indent => "Indent",
            Self::Delete => "Delete",
            Self::Comment => "Comment",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Copy => "Copy",
            Self::Cut => "Cut",
            Self::Paste => "Paste",
        }
    }
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Typing => "typing",
            Self::Newline => "newline",
            Self::Indent => "indent",
            Self::Delete => "delete",
            Self::Comment => "comment",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Copy => "copy",
            Self::Cut => "cut",
            Self::Paste => "paste",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ActionEffectSettings {
    pub(crate) particles: Option<ParticleStyle>,
    pub(crate) shake: bool,
    pub(crate) flash: bool,
    pub(crate) glow: bool,
}
impl Default for ActionEffectSettings {
    fn default() -> Self {
        Self {
            particles: Some(ParticleStyle::Explosion),
            shake: false,
            flash: false,
            glow: false,
        }
    }
}
impl ActionEffectSettings {
    pub(crate) const fn enabled(self) -> bool {
        self.particles.is_some() || self.shake || self.flash || self.glow
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct EditorActionEffects {
    pub(crate) typing: ActionEffectSettings,
    pub(crate) newline: ActionEffectSettings,
    pub(crate) indent: ActionEffectSettings,
    pub(crate) delete: ActionEffectSettings,
    pub(crate) comment: ActionEffectSettings,
    pub(crate) undo: ActionEffectSettings,
    pub(crate) redo: ActionEffectSettings,
    pub(crate) copy: ActionEffectSettings,
    pub(crate) cut: ActionEffectSettings,
    pub(crate) paste: ActionEffectSettings,
}
impl EditorActionEffects {
    pub(crate) const fn get(&self, action: EditorAction) -> &ActionEffectSettings {
        match action {
            EditorAction::Typing => &self.typing,
            EditorAction::Newline => &self.newline,
            EditorAction::Indent => &self.indent,
            EditorAction::Delete => &self.delete,
            EditorAction::Comment => &self.comment,
            EditorAction::Undo => &self.undo,
            EditorAction::Redo => &self.redo,
            EditorAction::Copy => &self.copy,
            EditorAction::Cut => &self.cut,
            EditorAction::Paste => &self.paste,
        }
    }
    pub(crate) const fn get_mut(&mut self, action: EditorAction) -> &mut ActionEffectSettings {
        match action {
            EditorAction::Typing => &mut self.typing,
            EditorAction::Newline => &mut self.newline,
            EditorAction::Indent => &mut self.indent,
            EditorAction::Delete => &mut self.delete,
            EditorAction::Comment => &mut self.comment,
            EditorAction::Undo => &mut self.undo,
            EditorAction::Redo => &mut self.redo,
            EditorAction::Copy => &mut self.copy,
            EditorAction::Cut => &mut self.cut,
            EditorAction::Paste => &mut self.paste,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct EditorEffectSettings {
    pub(crate) enabled: bool,
    pub(crate) density: ParticleDensity,
    pub(crate) actions: EditorActionEffects,
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
    #[serde(default)]
    pub(crate) command: crate::tool_command::CommandCustomization,
    pub(crate) mode: ToolMode,
    pub(crate) custom_path: String,
}

impl Default for ToolPreference {
    fn default() -> Self {
        Self {
            command: Default::default(),
            mode: ToolMode::Bundled,
            custom_path: String::new(),
        }
    }
}

impl ToolPreference {
    pub(crate) fn for_tool(kind: crate::toolchain::ToolKind) -> Self {
        Self {
            command: crate::tool_command::CommandCustomization::for_tool(kind),
            ..Default::default()
        }
    }

    fn deserialize_for<'de, D: serde::Deserializer<'de>>(
        kind: crate::toolchain::ToolKind,
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        if let Some(object) = value.as_object_mut() {
            let command = object
                .entry("command")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(command) = command.as_object_mut() {
                command
                    .entry("argv")
                    .or_insert_with(|| serde_json::json!(kind.default_arguments()));
            }
        }
        serde_path_to_error::deserialize(value).map_err(|error| {
            serde::de::Error::custom(format!(
                "{}.{}: {}",
                kind.binary_name(),
                error.path(),
                error.inner()
            ))
        })
    }
}

macro_rules! tool_preference_deserializer {
    ($name:ident, $kind:ident) => {
        pub(crate) fn $name<'de, D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> Result<ToolPreference, D::Error> {
            ToolPreference::deserialize_for(crate::toolchain::ToolKind::$kind, deserializer)
        }
    };
}
tool_preference_deserializer!(deserialize_typst_preference, Typst);
tool_preference_deserializer!(deserialize_tinymist_preference, Tinymist);
tool_preference_deserializer!(deserialize_tectonic_preference, Tectonic);
tool_preference_deserializer!(deserialize_texlab_preference, Texlab);
tool_preference_deserializer!(deserialize_badness_preference, Badness);
tool_preference_deserializer!(deserialize_tex_fmt_preference, TexFmt);

impl PreviewPreference {
    pub(crate) const fn is_interactive(self) -> bool {
        matches!(self, Self::Interactive | Self::InteractivePartial)
    }

    pub(crate) const ALL: [Self; 3] = [Self::Interactive, Self::InteractivePartial, Self::Pdfium];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Interactive => "Tinymist (Typst only)",
            Self::InteractivePartial => "Tinymist partial (experimental)",
            Self::Pdfium => "PDFium",
        }
    }
}

/// Presentation used for Git diffs in the Explorer and hunk popups.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum GitDiffStyle {
    /// A conventional unified diff with one source column.
    #[default]
    Unified,
    /// Old and new lines are shown in two aligned columns.
    SideBySide,
}

impl GitDiffStyle {
    pub(crate) const ALL: [Self; 2] = [Self::Unified, Self::SideBySide];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unified => "Single column",
            Self::SideBySide => "Side-by-side",
        }
    }
}

fn default_indent_spaces() -> u8 {
    2
}

/// Auto uses literal top-level Typst text language settings; English defaults to UK.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WritingLanguage {
    #[default]
    Auto,
    British,
    American,
}
impl WritingLanguage {
    pub(crate) const ALL: [Self; 3] = [Self::Auto, Self::British, Self::American];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (document; UK fallback)",
            Self::British => "English (UK)",
            Self::American => "English (US)",
        }
    }
}

/// Exactly one offline recognizer runs for each drawing, in either document language.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HandwritingEngine {
    Detypify,
    #[default]
    Detexify,
}
impl HandwritingEngine {
    pub(crate) const ALL: [Self; 2] = [Self::Detypify, Self::Detexify];
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Detypify => "Detypify",
            Self::Detexify => "Detexify",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct HandwritingSettings {
    pub(crate) engine: HandwritingEngine,
    pub(crate) prefer_typst_names: bool,
    pub(crate) prefer_math_mode: bool,
}
impl Default for HandwritingSettings {
    fn default() -> Self {
        Self {
            engine: HandwritingEngine::default(),
            prefer_typst_names: true,
            prefer_math_mode: true,
        }
    }
}

/// Persisted user choices for the current settings schema.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ToolbarStyle {
    Text,
    TextAndIcons,
    #[default]
    Icons,
}

impl ToolbarStyle {
    pub(crate) const ALL: [Self; 3] = [Self::Text, Self::TextAndIcons, Self::Icons];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Text => "Text only",
            Self::TextAndIcons => "Text and icons",
            Self::Icons => "Icons only",
        }
    }
}

/// Persisted user choices for the current settings schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AppSettings {
    pub(crate) interface_theme: InterfaceTheme,
    #[serde(default)]
    pub(crate) window_background: crate::window_background::Settings,
    #[serde(default)]
    pub(crate) source_reveal_animation: RevealAnimation,
    #[serde(default)]
    pub(crate) preview_reveal_animation: RevealAnimation,
    #[serde(default)]
    pub(crate) editor_effects: EditorEffectSettings,
    /// Color scheme used whenever the effective interface appearance is light.
    pub(crate) light_theme: ColorThemeChoice,
    /// Color scheme used whenever the effective interface appearance is dark.
    pub(crate) dark_theme: ColorThemeChoice,
    /// Applied before hue rotation to the complete semantic and syntax palette.
    pub(crate) theme_invert: bool,
    /// Whole-theme hue rotation, in degrees, applied after optional inversion.
    pub(crate) theme_hue_shift_degrees: i16,
    #[serde(default)]
    pub(crate) theme_colors: crate::theme_transform::ThemeColorAdjustments,
    pub(crate) document_theme: DocumentTheme,
    /// Apply the selected dark page appearance to opened images.
    #[serde(default = "default_true")]
    pub(crate) dark_images: bool,
    #[serde(default = "default_true")]
    pub(crate) comfy_background: bool,
    #[serde(default)]
    pub(crate) comfy_text: bool,
    #[serde(default)]
    pub(crate) preview_preference: PreviewPreference,
    #[serde(default = "default_true")]
    pub(crate) preview_zoom_snapshot: bool,
    #[serde(default)]
    pub(crate) git_diff_style: GitDiffStyle,
    pub(crate) line_wrap: bool,
    #[serde(default)]
    pub(crate) independent_main_dividers: bool,
    #[serde(default = "crate::editor_decoration::enabled")]
    pub(crate) indentation_guides: bool,
    #[serde(default = "crate::editor_decoration::default_guide")]
    pub(crate) indentation_guide_character: String,
    #[serde(default)]
    pub(crate) highlight_local_uses: bool,
    #[serde(default = "crate::editor_decoration::default_rules")]
    pub(crate) highlight_rules: Vec<crate::editor_decoration::HighlightRule>,
    pub(crate) line_numbers: bool,
    #[serde(default)]
    pub(crate) english_grammar: bool,
    #[serde(default)]
    pub(crate) snippets: Vec<crate::snippets::Snippet>,
    #[serde(default = "crate::snippet_presets::defaults")]
    pub(crate) snippet_presets: Vec<crate::snippets::Snippet>,
    #[serde(default = "crate::document_templates::defaults")]
    pub(crate) templates: Vec<crate::document_templates::Template>,
    #[serde(default)]
    pub(crate) writing_language: WritingLanguage,
    #[serde(default)]
    pub(crate) harper_dictionary: String,
    #[serde(default)]
    pub(crate) harper_ignore_capitalized_names: bool,
    #[serde(default)]
    pub(crate) harper_rule_overrides: crate::writing::RuleOverrides,
    #[serde(default)]
    pub(crate) tex_prose_rules: crate::writing::prose_rules::TexProseRules,
    #[serde(default)]
    pub(crate) ltex: crate::ltex::LtexSettings,
    #[serde(default)]
    pub(crate) liquid_cache_directory: String,
    #[serde(default = "default_liquid_threads")]
    pub(crate) liquid_threads: u8,
    #[serde(default)]
    pub(crate) liquid_input: crate::liquid_grammar::InputOptions,
    #[serde(default)]
    pub(crate) handwriting: HandwritingSettings,
    #[serde(default)]
    pub(crate) unicode_warnings: bool,
    #[serde(default = "default_true")]
    pub(crate) sticky_context_rows: bool,
    #[serde(default = "default_true")]
    pub(crate) auto_pair_delimiters: bool,
    #[serde(default = "default_indent_spaces")]
    pub(crate) indent_spaces: u8,
    #[serde(default)]
    pub(crate) ascii_punctuation: bool,
    #[serde(default = "default_true")]
    pub(crate) format_after_math_conversion: bool,
    #[serde(default)]
    pub(crate) mitex_auto_enable: bool,
    #[serde(default = "default_mitex_version")]
    pub(crate) mitex_version: String,
    #[serde(default)]
    pub(crate) rainbow_brackets: crate::rainbow::RainbowBrackets,
    #[serde(default)]
    pub(crate) explorer_order: ExplorerOrder,
    pub(crate) source_preview_trigger: SourcePreviewTrigger,
    #[serde(default = "default_true")]
    pub(crate) preview_follow_edits: bool,
    #[serde(default)]
    pub(crate) tex_preview_follow_edits: bool,
    pub(crate) auto_save: bool,
    pub(crate) auto_save_delay_ms: u64,
    pub(crate) hover_delay_ms: u64,
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
    #[serde(default)]
    pub(crate) toolbar_style: ToolbarStyle,
    /// Group the five workspace layout actions in one title-bar menu.
    #[serde(default = "default_true")]
    pub(crate) compact_layout_controls: bool,
    /// Give every document tab the same width instead of sizing it to its name.
    #[serde(default)]
    pub(crate) fixed_tab_width: bool,
    /// User changes from the current platform's built-in command bindings.
    /// Action IDs and chords are normalized after deserialization.
    #[serde(default, skip_serializing_if = "ShortcutOverrides::is_empty")]
    pub(crate) shortcut_overrides: ShortcutOverrides,
    /// Optional Typst-only style layers for each interface appearance.
    pub(crate) typst_overrides: TypstOverrideThemes,
    /// Verified Overleaf project URLs, newest first. Authentication stays in WebKit.
    #[serde(default, deserialize_with = "deserialize_overleaf_projects")]
    pub(crate) overleaf_projects: Vec<String>,
    /// Most recently used canonical workspace roots, newest first.
    pub(crate) recent_workspaces: Vec<String>,
    /// Last successfully opened source document, keyed by canonical project root.
    pub(crate) last_opened_files: BTreeMap<String, String>,
    #[serde(deserialize_with = "deserialize_typst_preference")]
    pub(crate) typst: ToolPreference,
    #[serde(deserialize_with = "deserialize_tinymist_preference")]
    pub(crate) tinymist: ToolPreference,
    #[serde(default = "crate::tinymist::default_server_configuration")]
    pub(crate) tinymist_configuration: serde_json::Value,
    #[serde(default)]
    pub(crate) tex: crate::tex::settings::TexSettings,
    /// Independent comparison tool: changing these options must not restart TeX services.
    #[serde(default)]
    pub(crate) latexdiff: crate::tex::diff::Settings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            interface_theme: InterfaceTheme::System,
            window_background: crate::window_background::Settings::default(),
            source_reveal_animation: RevealAnimation::default(),
            preview_reveal_animation: RevealAnimation::default(),
            editor_effects: EditorEffectSettings::default(),
            light_theme: ColorThemeChoice::builtin("tiptop-light"),
            dark_theme: ColorThemeChoice::builtin("tiptop-dark"),
            theme_invert: false,
            theme_hue_shift_degrees: 0,
            theme_colors: Default::default(),
            comfy_background: true,
            comfy_text: false,
            document_theme: DocumentTheme::FollowInterface,
            dark_images: true,
            preview_preference: PreviewPreference::default(),
            preview_zoom_snapshot: true,
            git_diff_style: GitDiffStyle::Unified,
            line_wrap: true,
            independent_main_dividers: false,
            indentation_guides: true,
            indentation_guide_character: crate::editor_decoration::default_guide(),
            highlight_local_uses: false,
            highlight_rules: crate::editor_decoration::default_rules(),
            line_numbers: true,
            english_grammar: false,
            snippets: Vec::new(),
            snippet_presets: crate::snippet_presets::defaults(),
            templates: crate::document_templates::defaults(),
            writing_language: WritingLanguage::Auto,
            harper_dictionary: String::new(),
            harper_ignore_capitalized_names: false,
            harper_rule_overrides: Default::default(),
            tex_prose_rules: Default::default(),
            ltex: Default::default(),
            liquid_cache_directory: String::new(),
            liquid_threads: default_liquid_threads(),
            liquid_input: Default::default(),
            handwriting: HandwritingSettings::default(),
            unicode_warnings: false,
            sticky_context_rows: true,
            auto_pair_delimiters: true,
            indent_spaces: 2,
            ascii_punctuation: false,
            format_after_math_conversion: true,
            mitex_auto_enable: false,
            mitex_version: default_mitex_version(),
            rainbow_brackets: crate::rainbow::RainbowBrackets::default(),
            explorer_order: ExplorerOrder::default(),
            source_preview_trigger: SourcePreviewTrigger::DoubleClick,
            preview_follow_edits: true,
            tex_preview_follow_edits: false,
            auto_save: true,
            auto_save_delay_ms: 750,
            hover_delay_ms: DEFAULT_HOVER_DELAY_MS,
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
            titlebar_menus: false,
            toolbar_style: ToolbarStyle::default(),
            compact_layout_controls: true,
            fixed_tab_width: false,
            shortcut_overrides: ShortcutOverrides::default(),
            typst_overrides: TypstOverrideThemes::default(),
            overleaf_projects: Vec::new(),
            recent_workspaces: Vec::new(),
            last_opened_files: BTreeMap::new(),
            typst: ToolPreference::for_tool(crate::toolchain::ToolKind::Typst),
            tinymist: ToolPreference::for_tool(crate::toolchain::ToolKind::Tinymist),
            tinymist_configuration: crate::tinymist::default_server_configuration(),
            tex: crate::tex::settings::TexSettings::default(),
            latexdiff: crate::tex::diff::Settings::default(),
        }
    }
}

const fn default_true() -> bool {
    true
}

fn default_mitex_version() -> String {
    "0.2.7".into()
}

impl AppSettings {
    /// Apply only the fields changed in an independently rendered Settings
    /// window. Preserve newer document history and edits from sibling windows.
    pub(crate) fn apply_edits(&mut self, base: &Self, edited: Self) {
        macro_rules! merge {
            ($($field:ident),+ $(,)?) => {
                // Exhaustive destructuring makes adding a setting without
                // considering deferred edits a compile error.
                let Self { $($field),+ } = edited;
                $(if $field != base.$field { self.$field = $field; })+
            };
        }
        merge!(
            interface_theme,
            window_background,
            source_reveal_animation,
            preview_reveal_animation,
            editor_effects,
            light_theme,
            dark_theme,
            theme_invert,
            theme_hue_shift_degrees,
            theme_colors,
            document_theme,
            dark_images,
            comfy_background,
            comfy_text,
            preview_preference,
            preview_zoom_snapshot,
            git_diff_style,
            line_wrap,
            independent_main_dividers,
            indentation_guides,
            indentation_guide_character,
            highlight_local_uses,
            highlight_rules,
            line_numbers,
            english_grammar,
            snippets,
            snippet_presets,
            templates,
            writing_language,
            harper_dictionary,
            harper_ignore_capitalized_names,
            harper_rule_overrides,
            tex_prose_rules,
            ltex,
            liquid_cache_directory,
            liquid_threads,
            liquid_input,
            handwriting,
            unicode_warnings,
            sticky_context_rows,
            auto_pair_delimiters,
            indent_spaces,
            ascii_punctuation,
            format_after_math_conversion,
            mitex_auto_enable,
            mitex_version,
            rainbow_brackets,
            explorer_order,
            source_preview_trigger,
            preview_follow_edits,
            tex_preview_follow_edits,
            auto_save,
            auto_save_delay_ms,
            hover_delay_ms,
            ui_scale_percent,
            ui_font_monospace,
            ui_font_path,
            ui_font_family,
            ui_font_face_index,
            ui_font_weight,
            code_font_path,
            code_font_family,
            code_font_face_index,
            code_font_weight,
            titlebar_menus,
            toolbar_style,
            compact_layout_controls,
            fixed_tab_width,
            shortcut_overrides,
            typst_overrides,
            overleaf_projects,
            recent_workspaces,
            last_opened_files,
            typst,
            tinymist,
            tinymist_configuration,
            tex,
            latexdiff,
        );
    }

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

    /// Remember a successfully connected project without persisting browser
    /// cookies, credentials, or a URL containing an access token.
    pub(crate) fn remember_overleaf_project(&mut self, url: &str) -> Result<(), String> {
        let project = crate::overleaf::ProjectUrl::parse(url)?;
        let canonical = project.url().to_owned();
        let mut projects = normalize_overleaf_projects(&self.overleaf_projects)?;
        projects.retain(|value| value != &canonical);
        projects.insert(0, canonical);
        projects.truncate(8);
        self.overleaf_projects = projects;
        Ok(())
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

    pub(crate) fn load(storage: Option<&dyn Storage>) -> Result<Self, SettingsLoadError> {
        let Some(serialized) = storage.and_then(|storage| storage.get_string(STORAGE_KEY)) else {
            return Ok(Self::default());
        };
        let mut settings: Self =
            serde_json::from_str(&serialized).map_err(|error| SettingsLoadError {
                message: error.to_string(),
                rejected: serialized,
            })?;
        settings.ui_font_weight = settings.ui_font_weight.clamp(1, 1_000);
        settings.code_font_weight = settings.code_font_weight.clamp(1, 1_000);
        settings.normalize_builtin_theme_slots();
        settings.theme_colors = settings.theme_colors.normalized();
        settings.source_reveal_animation = settings.source_reveal_animation.normalized();
        settings.preview_reveal_animation = settings.preview_reveal_animation.normalized();
        settings.normalize_workspace_history();
        settings.shortcut_overrides.normalize();
        Ok(settings)
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
    }

    fn normalize_builtin_theme_slots(&mut self) {
        for (choice, dark_mode) in [(&mut self.light_theme, false), (&mut self.dark_theme, true)] {
            let valid = match choice {
                ColorThemeChoice::Builtin(id) => builtin_themes::find(id).is_some(),
                ColorThemeChoice::Sublime(_) => true,
            };
            if !valid {
                *choice = ColorThemeChoice::builtin(builtin_themes::default_for_mode(dark_mode).id);
            }
        }
    }

    pub(crate) fn save(&self, storage: &mut dyn Storage) {
        let mut normalized = self.clone();
        normalized.theme_colors = normalized.theme_colors.normalized();
        normalized.source_reveal_animation = normalized.source_reveal_animation.normalized();
        normalized.preview_reveal_animation = normalized.preview_reveal_animation.normalized();
        normalized.normalize_workspace_history();
        normalized.shortcut_overrides.normalize();
        if let Ok(serialized) = serde_json::to_string(&normalized) {
            // Preserve rejected input before replacing it, including saves
            // routed through the multi-window shell rather than EditorApp.
            if let Err(error) = Self::load(Some(storage)) {
                storage.set_string(REJECTED_STORAGE_KEY, error.rejected);
            }
            storage.set_string(STORAGE_KEY, serialized);
        }
    }

    pub(crate) fn effective_shortcuts(&self) -> ShortcutBindings {
        ShortcutBindings::current(&self.shortcut_overrides)
    }
}

fn deserialize_overleaf_projects<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    normalize_overleaf_projects(&values).map_err(serde::de::Error::custom)
}

fn normalize_overleaf_projects(values: &[String]) -> Result<Vec<String>, String> {
    let mut projects = Vec::new();
    // Validate every supplied entry, including those beyond the history limit;
    // invalid options should produce an error instead of disappearing silently.
    for (index, value) in values.iter().enumerate() {
        let project = crate::overleaf::ProjectUrl::parse(value)
            .map_err(|error| format!("Overleaf project {}: {error}", index + 1))?;
        if projects.len() < 8 && !projects.iter().any(|url| url == project.url()) {
            projects.push(project.url().to_owned());
        }
    }
    Ok(projects)
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
    fn viewport_zoom_cache_defaults_on_and_can_be_overridden_in_json() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(value["preview_zoom_snapshot"], true);
        value["preview_zoom_snapshot"] = false.into();
        let disabled = serde_json::from_value::<AppSettings>(value.clone()).unwrap();
        assert!(!disabled.preview_zoom_snapshot);
        value
            .as_object_mut()
            .unwrap()
            .remove("preview_zoom_snapshot");
        assert!(
            serde_json::from_value::<AppSettings>(value)
                .unwrap()
                .preview_zoom_snapshot
        );
    }

    #[test]
    fn preview_backends_have_no_retired_compatibility_aliases() {
        assert_eq!(
            PreviewPreference::default(),
            PreviewPreference::InteractivePartial
        );
        for retired in ["PdfJs", "Native"] {
            assert!(
                serde_json::from_value::<PreviewPreference>(serde_json::json!(retired)).is_err()
            );
        }
        assert_eq!(PreviewPreference::ALL.len(), 3);
    }

    #[test]
    fn partial_tinymist_is_a_separate_persisted_preview_choice() {
        assert_eq!(
            PreviewPreference::default(),
            PreviewPreference::InteractivePartial
        );
        assert!(PreviewPreference::Interactive.is_interactive());
        assert!(PreviewPreference::InteractivePartial.is_interactive());
        assert!(!PreviewPreference::Pdfium.is_interactive());
        assert_ne!(
            PreviewPreference::Interactive.label(),
            PreviewPreference::InteractivePartial.label()
        );
        assert!(
            PreviewPreference::InteractivePartial
                .label()
                .contains("experimental")
        );
        for preference in PreviewPreference::ALL {
            let json = serde_json::to_value(preference).unwrap();
            assert_eq!(
                serde_json::from_value::<PreviewPreference>(json).unwrap(),
                preference
            );
        }
    }

    #[test]
    fn preview_default_applies_only_when_no_choice_is_stored() {
        assert_eq!(
            AppSettings::load(None).unwrap().preview_preference,
            PreviewPreference::InteractivePartial
        );
        let mut storage = MemoryStorage::default();
        assert_eq!(
            AppSettings::load(Some(&storage))
                .unwrap()
                .preview_preference,
            PreviewPreference::InteractivePartial
        );
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        value.as_object_mut().unwrap().remove("preview_preference");
        storage.set_string(STORAGE_KEY, value.to_string());
        assert_eq!(
            AppSettings::load(Some(&storage))
                .unwrap()
                .preview_preference,
            PreviewPreference::InteractivePartial
        );
        assert_eq!(
            crate::settings_json::parse(&value.to_string())
                .unwrap()
                .preview_preference,
            PreviewPreference::InteractivePartial
        );
        for preference in PreviewPreference::ALL {
            value["preview_preference"] = serde_json::to_value(preference).unwrap();
            storage.set_string(STORAGE_KEY, value.to_string());
            assert_eq!(
                AppSettings::load(Some(&storage))
                    .unwrap()
                    .preview_preference,
                preference
            );
            assert_eq!(
                crate::settings_json::parse(&value.to_string())
                    .unwrap()
                    .preview_preference,
                preference
            );
        }
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
        assert!(settings.dark_images);
        assert!(settings.comfy_background);
        assert!(!settings.comfy_text);
        assert!(!settings.titlebar_menus);
        let mut stored = serde_json::to_value(&settings).unwrap();
        stored.as_object_mut().unwrap().remove("dark_images");
        stored.as_object_mut().unwrap().remove("comfy_background");
        let omitted: AppSettings = serde_json::from_value(stored.clone()).unwrap();
        assert!(omitted.dark_images && omitted.comfy_background);
        stored["dark_images"] = false.into();
        stored["comfy_background"] = false.into();
        stored["titlebar_menus"] = true.into();
        let explicit: AppSettings = serde_json::from_value(stored).unwrap();
        assert!(!explicit.dark_images && !explicit.comfy_background);
        assert!(explicit.titlebar_menus);
        assert_eq!(
            settings.preview_preference,
            PreviewPreference::InteractivePartial
        );
        assert_eq!(settings.indent_spaces, 2);
        assert!(!settings.ascii_punctuation);
        assert!(settings.line_wrap);
        assert!(settings.line_numbers);
        assert_eq!(
            settings.source_preview_trigger,
            SourcePreviewTrigger::DoubleClick
        );
        assert!(settings.auto_save);
        assert!(settings.preview_follow_edits);
        assert!(!settings.tex_preview_follow_edits);
        assert_eq!(settings.auto_save_delay_ms, 750);
        assert!(!settings.fixed_tab_width);
        assert_eq!(settings.hover_delay_ms, DEFAULT_HOVER_DELAY_MS);
        assert_eq!(settings.typst_overrides, TypstOverrideThemes::default());
        assert!(settings.shortcut_overrides.is_empty());
        assert!(settings.effective_shortcuts().conflicts().is_empty());
        assert!(settings.last_opened_files.is_empty());
        assert!(settings.recent_workspaces.is_empty());
        assert!(settings.overleaf_projects.is_empty());
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
            window_background: crate::window_background::Settings::default(),
            source_reveal_animation: RevealAnimation::default(),
            preview_reveal_animation: RevealAnimation::default(),
            editor_effects: EditorEffectSettings::default(),
            tex: Default::default(),
            latexdiff: Default::default(),
            interface_theme: InterfaceTheme::Light,
            light_theme: ColorThemeChoice::builtin("catppuccin-latte"),
            dark_theme: ColorThemeChoice::sublime("/themes/Example.sublime-color-scheme"),
            theme_invert: true,
            theme_hue_shift_degrees: -45,
            theme_colors: crate::theme_transform::ThemeColorAdjustments {
                luminosity: 20,
                brightness: -10,
                contrast: 115,
                saturation: 80,
            },
            comfy_background: false,
            comfy_text: false,
            document_theme: DocumentTheme::Dark,
            dark_images: true,
            preview_preference: PreviewPreference::Pdfium,
            preview_zoom_snapshot: false,
            git_diff_style: GitDiffStyle::Unified,
            line_wrap: false,
            independent_main_dividers: true,
            indentation_guides: false,
            indentation_guide_character: "┆".into(),
            highlight_local_uses: true,
            highlight_rules: crate::editor_decoration::default_rules(),
            line_numbers: false,
            english_grammar: false,
            snippets: Vec::new(),
            snippet_presets: crate::snippet_presets::defaults(),
            templates: crate::document_templates::defaults(),
            writing_language: WritingLanguage::Auto,
            harper_dictionary: String::new(),
            harper_ignore_capitalized_names: false,
            harper_rule_overrides: Default::default(),
            tex_prose_rules: Default::default(),
            ltex: Default::default(),
            liquid_cache_directory: String::new(),
            liquid_threads: default_liquid_threads(),
            liquid_input: Default::default(),
            handwriting: HandwritingSettings::default(),
            unicode_warnings: false,
            sticky_context_rows: false,
            auto_pair_delimiters: false,
            indent_spaces: 4,
            ascii_punctuation: true,
            format_after_math_conversion: false,
            mitex_auto_enable: true,
            mitex_version: "0.2.7".into(),
            rainbow_brackets: crate::rainbow::RainbowBrackets {
                enabled: false,
                palettes: [crate::rainbow::BracketPalette::Orchid; 4],
            },
            explorer_order: {
                let mut order = ExplorerOrder::default();
                order.move_to(crate::explorer::ExplorerSection::Tags, 0);
                order
            },
            source_preview_trigger: SourcePreviewTrigger::ModifierClick,
            preview_follow_edits: false,
            tex_preview_follow_edits: true,
            auto_save: false,
            auto_save_delay_ms: 1_500,
            hover_delay_ms: 450,
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
            toolbar_style: ToolbarStyle::Icons,
            compact_layout_controls: false,
            fixed_tab_width: true,
            shortcut_overrides: ShortcutOverrides::default(),
            typst_overrides,
            last_opened_files: BTreeMap::from([(
                "/workspace".to_owned(),
                "/workspace/main.typ".to_owned(),
            )]),
            overleaf_projects: vec![
                "https://www.overleaf.com/project/0123456789abcdef01234567".into(),
            ],
            recent_workspaces: vec!["/workspace".to_owned()],
            typst: ToolPreference {
                command: Default::default(),
                mode: ToolMode::Custom,
                custom_path: "/opt/typst".to_owned(),
            },
            tinymist: ToolPreference {
                command: Default::default(),
                mode: ToolMode::Custom,
                custom_path: "/opt/tinymist".to_owned(),
            },
            tinymist_configuration: serde_json::json!({"formatterPrintWidth":100}),
        };
        let mut storage = MemoryStorage::default();
        expected.save(&mut storage);
        assert_eq!(AppSettings::load(Some(&storage)).unwrap(), expected);

        storage.set_string(STORAGE_KEY, r#"{"interface_theme":"Dark"}"#.to_owned());
        assert!(AppSettings::load(Some(&storage)).is_err());
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

        let settings = AppSettings::load(Some(&storage)).unwrap();
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

        let restored = AppSettings::load(Some(&storage)).unwrap();
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

        let settings = AppSettings::load(Some(&storage)).unwrap();
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
            light_theme: ColorThemeChoice::builtin("catppuccin-mocha"),
            dark_theme: ColorThemeChoice::builtin("paper-light"),
            ..AppSettings::default()
        };
        let mut storage = MemoryStorage::default();
        settings.save(&mut storage);
        settings.interface_theme = InterfaceTheme::System;

        assert_eq!(AppSettings::load(Some(&storage)).unwrap(), settings);
    }

    #[test]
    fn only_unknown_builtin_slots_use_their_matching_defaults() {
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

        let settings = AppSettings::load(Some(&storage)).unwrap();
        assert_eq!(
            settings.light_theme,
            ColorThemeChoice::builtin("catppuccin-mocha")
        );
        assert_eq!(
            settings.dark_theme,
            ColorThemeChoice::builtin("tiptop-dark")
        );
    }

    #[test]
    fn color_adjustments_are_normalized_without_discarding_other_preferences() {
        let mut storage = MemoryStorage::default();
        let settings = AppSettings {
            auto_save: false,
            theme_colors: crate::theme_transform::ThemeColorAdjustments {
                luminosity: -500,
                brightness: 300,
                contrast: 0,
                saturation: 999,
            },
            ..Default::default()
        };
        storage.set_string(STORAGE_KEY, serde_json::to_string(&settings).unwrap());
        let restored = AppSettings::load(Some(&storage)).unwrap();
        assert_eq!(restored.theme_colors, settings.theme_colors.normalized());
        assert!(!restored.auto_save);
        settings.save(&mut storage);
        assert_eq!(AppSettings::load(Some(&storage)).unwrap(), restored);
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
        let restored = AppSettings::load(Some(&storage)).unwrap();

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
    fn missing_settings_are_normal_but_corrupt_input_is_reported_and_preserved() {
        let mut storage = MemoryStorage::default();
        assert_eq!(AppSettings::load(None).unwrap(), AppSettings::default());
        assert_eq!(
            AppSettings::load(Some(&storage)).unwrap(),
            AppSettings::default()
        );
        for rejected in ["not json", r#"{"interface_theme":"unknown"}"#] {
            storage.set_string(STORAGE_KEY, rejected.to_owned());
            let error = AppSettings::load(Some(&storage)).unwrap_err();
            assert_eq!(error.rejected, rejected);
            assert!(
                error
                    .to_string()
                    .contains("Saved settings could not be read")
            );
            assert_eq!(storage.get_string(STORAGE_KEY).as_deref(), Some(rejected));
            AppSettings::default().save(&mut storage);
            assert_eq!(
                storage.get_string(REJECTED_STORAGE_KEY).as_deref(),
                Some(rejected)
            );
            assert_eq!(
                AppSettings::load(Some(&storage)).unwrap(),
                AppSettings::default()
            );
            AppSettings::default().save(&mut storage);
            assert_eq!(
                storage.get_string(REJECTED_STORAGE_KEY).as_deref(),
                Some(rejected)
            );
        }
    }

    #[test]
    fn git_diff_style_persists_and_old_settings_default_to_single_column() {
        let mut storage = MemoryStorage::default();
        let settings = AppSettings {
            git_diff_style: GitDiffStyle::SideBySide,
            ..AppSettings::default()
        };
        settings.save(&mut storage);
        assert_eq!(
            AppSettings::load(Some(&storage)).unwrap().git_diff_style,
            GitDiffStyle::SideBySide
        );

        let mut value: serde_json::Value = serde_json::from_str(
            &storage
                .get_string(STORAGE_KEY)
                .expect("settings were persisted"),
        )
        .unwrap();
        value.as_object_mut().unwrap().remove("git_diff_style");
        storage.set_string(STORAGE_KEY, serde_json::to_string(&value).unwrap());
        assert_eq!(
            AppSettings::load(Some(&storage)).unwrap().git_diff_style,
            GitDiffStyle::Unified
        );
    }

    #[test]
    fn motion_settings_defaults_roundtrip_and_merge_independently() {
        let defaults = AppSettings::default();
        assert_eq!(
            defaults.source_reveal_animation,
            RevealAnimation {
                enabled: true,
                style: RevealStyle::Rings,
                duration_ms: 900,
                size_percent: 100
            }
        );
        assert_eq!(
            defaults.preview_reveal_animation,
            defaults.source_reveal_animation
        );
        assert_eq!(
            defaults.editor_effects,
            EditorEffectSettings {
                enabled: false,
                density: ParticleDensity::Medium,
                actions: EditorActionEffects::default()
            }
        );
        let mut value = serde_json::to_value(&defaults).unwrap();
        for key in [
            "source_reveal_animation",
            "preview_reveal_animation",
            "editor_effects",
        ] {
            value.as_object_mut().unwrap().remove(key);
        }
        assert_eq!(
            serde_json::from_value::<AppSettings>(value).unwrap(),
            defaults
        );
        let mut source = defaults.clone();
        source.source_reveal_animation = RevealAnimation {
            enabled: false,
            style: RevealStyle::Spotlight,
            duration_ms: 1250,
            size_percent: 150,
        };
        let mut preview = defaults.clone();
        preview.preview_reveal_animation.style = RevealStyle::Pulse;
        preview.editor_effects.enabled = true;
        preview.editor_effects.density = ParticleDensity::High;
        preview.editor_effects.actions.paste = ActionEffectSettings {
            particles: Some(ParticleStyle::Shotgun),
            shake: true,
            flash: true,
            glow: true,
        };
        let mut current = defaults.clone();
        current.apply_edits(&defaults, source);
        let effects = preview.editor_effects;
        current.apply_edits(&defaults, preview);
        assert!(!current.source_reveal_animation.enabled);
        assert_eq!(
            current.source_reveal_animation.style,
            RevealStyle::Spotlight
        );
        assert_eq!(current.preview_reveal_animation.style, RevealStyle::Pulse);
        assert!(current.editor_effects.enabled);
        assert_eq!(current.editor_effects.density, ParticleDensity::High);
        assert_eq!(current.editor_effects.actions.paste, effects.actions.paste);
        let mut storage = MemoryStorage::default();
        current.save(&mut storage);
        assert_eq!(AppSettings::load(Some(&storage)).unwrap(), current);
    }

    #[test]
    fn per_action_effects_persist_independently_and_allow_every_effect_to_be_disabled() {
        let mut settings = AppSettings::default();
        let mut storage = MemoryStorage::default();
        let off = ActionEffectSettings {
            particles: None,
            shake: false,
            flash: false,
            glow: false,
        };
        assert!(!off.enabled());
        for action in EditorAction::ALL {
            for style in ParticleStyle::ALL.into_iter().map(Some).chain([None]) {
                let effects = ActionEffectSettings {
                    particles: style,
                    shake: true,
                    flash: true,
                    glow: true,
                };
                assert!(effects.enabled());
                *settings.editor_effects.actions.get_mut(action) = effects;
                settings.save(&mut storage);
                let loaded = AppSettings::load(Some(&storage)).unwrap();
                assert_eq!(*loaded.editor_effects.actions.get(action), effects);
                assert_eq!(loaded, settings);
                assert!(!loaded.editor_effects.enabled);
            }
            *settings.editor_effects.actions.get_mut(action) = off;
        }
        assert!(
            EditorAction::ALL.into_iter().all(|action| !settings
                .editor_effects
                .actions
                .get(action)
                .enabled())
        );
        settings.save(&mut storage);
        assert_eq!(AppSettings::load(Some(&storage)).unwrap(), settings);

        let mut value = serde_json::to_value(&settings).unwrap();
        value["editor_effects"]
            .as_object_mut()
            .unwrap()
            .remove("actions");
        let loaded: AppSettings = serde_json::from_value(value).unwrap();
        assert_eq!(
            loaded.editor_effects.actions,
            EditorActionEffects::default()
        );
        assert!(!loaded.editor_effects.enabled);
    }

    #[test]
    fn persisted_motion_ranges_are_normalized_without_enabling_effects() {
        let invalid = AppSettings {
            source_reveal_animation: RevealAnimation {
                enabled: false,
                duration_ms: 0,
                size_percent: 65535,
                ..Default::default()
            },
            preview_reveal_animation: RevealAnimation {
                duration_ms: 65535,
                size_percent: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut storage = MemoryStorage::default();
        storage.set_string(STORAGE_KEY, serde_json::to_string(&invalid).unwrap());
        let loaded = AppSettings::load(Some(&storage)).unwrap();
        assert_eq!(
            (
                loaded.source_reveal_animation.duration_ms,
                loaded.source_reveal_animation.size_percent
            ),
            (250, 200)
        );
        assert_eq!(
            (
                loaded.preview_reveal_animation.duration_ms,
                loaded.preview_reveal_animation.size_percent
            ),
            (2000, 50)
        );
        assert!(!loaded.source_reveal_animation.enabled);
        assert!(!loaded.editor_effects.enabled);
        invalid.save(&mut storage);
        assert_eq!(AppSettings::load(Some(&storage)).unwrap(), loaded);
    }

    #[test]
    fn harper_rule_profile_roundtrips_rejects_invalid_json_and_merges_independently() {
        let base = AppSettings::default();
        assert!(!base.english_grammar);
        assert!(!base.harper_ignore_capitalized_names);
        let mut edited = base.clone();
        let mut rules = serde_json::to_value(&edited.harper_rule_overrides).unwrap();
        rules["NoFrenchSpaces"] = serde_json::json!(true);
        edited.harper_rule_overrides = serde_json::from_value(rules).unwrap();
        let saved = serde_json::to_string(&edited).unwrap();
        assert_eq!(crate::settings_json::parse(&saved).unwrap(), edited);
        let mut current = base.clone();
        current.harper_dictionary = "Romito".into();
        current.writing_language = WritingLanguage::American;
        current.apply_edits(&base, edited.clone());
        assert_eq!(current.harper_rule_overrides, edited.harper_rule_overrides);
        assert_eq!(current.harper_dictionary, "Romito");
        assert_eq!(current.writing_language, WritingLanguage::American);
        let mut other_edit = base.clone();
        other_edit.harper_ignore_capitalized_names = true;
        current.apply_edits(&base, other_edit);
        assert_eq!(
            current.harper_rule_overrides, edited.harper_rule_overrides,
            "a stale window that did not edit rules must retain newer overrides"
        );
        let mut value = serde_json::to_value(&base).unwrap();
        value["harper_rule_overrides"] = serde_json::json!({"SpellChek": false});
        let error = crate::settings_json::parse(&value.to_string()).unwrap_err();
        assert!(error.contains("harper_rule_overrides") && error.contains("SpellChek"));
        value["harper_rule_overrides"] = serde_json::json!({"SpellCheck": "false"});
        assert!(
            crate::settings_json::parse(&value.to_string())
                .unwrap_err()
                .contains("harper_rule_overrides")
        );
    }

    #[test]
    fn harper_personal_settings_default_persist_and_merge_independently() {
        let original = AppSettings::default();
        assert!(original.harper_dictionary.is_empty());
        assert!(!original.harper_ignore_capitalized_names);
        let mut edited = original.clone();
        edited.harper_dictionary = "Qzorthal\nMacDonald\nÉvariste".into();
        edited.harper_ignore_capitalized_names = true;
        let decoded: AppSettings =
            serde_json::from_str(&serde_json::to_string(&edited).unwrap()).unwrap();
        assert_eq!(decoded, edited);
        let mut current = original.clone();
        current.writing_language = WritingLanguage::American;
        current.apply_edits(&original, edited);
        assert_eq!(current.writing_language, WritingLanguage::American);
        assert_eq!(current.harper_dictionary, "Qzorthal\nMacDonald\nÉvariste");
        assert!(current.harper_ignore_capitalized_names);
        let base = current.clone();
        let mut edited = base.clone();
        edited.harper_ignore_capitalized_names = false;
        current.harper_dictionary = "newer entry".into();
        current.apply_edits(&base, edited);
        assert_eq!(current.harper_dictionary, "newer entry");
        assert!(!current.harper_ignore_capitalized_names);
    }

    #[test]
    fn compact_layout_controls_default_on_persist_and_merge_without_overwriting_newer_edits() {
        let base = AppSettings::default();
        assert!(base.compact_layout_controls);
        let mut edited = base.clone();
        edited.compact_layout_controls = false;
        let mut storage = MemoryStorage::default();
        edited.save(&mut storage);
        assert!(
            !AppSettings::load(Some(&storage))
                .unwrap()
                .compact_layout_controls
        );

        let mut value = serde_json::to_value(&edited).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("compact_layout_controls");
        storage.set_string(STORAGE_KEY, value.to_string());
        assert!(
            AppSettings::load(Some(&storage))
                .unwrap()
                .compact_layout_controls
        );

        let mut current = base.clone();
        current.line_numbers = false;
        current.apply_edits(&base, edited);
        assert!(!current.compact_layout_controls);
        assert!(!current.line_numbers);
        let mut unrelated = base.clone();
        unrelated.auto_save = false;
        current.apply_edits(&base, unrelated);
        assert!(!current.compact_layout_controls);
    }

    #[test]
    fn preview_follow_edits_defaults_on_persists_off_and_merges_independently() {
        let base = AppSettings::default();
        let mut edited = base.clone();
        edited.preview_follow_edits = false;
        let mut storage = MemoryStorage::default();
        edited.save(&mut storage);
        assert!(
            !AppSettings::load(Some(&storage))
                .unwrap()
                .preview_follow_edits
        );

        let mut value = serde_json::to_value(&edited).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("preview_follow_edits");
        storage.set_string(STORAGE_KEY, value.to_string());
        assert!(
            AppSettings::load(Some(&storage))
                .unwrap()
                .preview_follow_edits
        );

        let mut current = base.clone();
        current.line_numbers = false;
        current.apply_edits(&base, edited);
        assert!(!current.preview_follow_edits);
        assert!(
            !current.line_numbers,
            "preserve other windows' newer settings"
        );
        let mut unrelated = base.clone();
        unrelated.auto_save = false;
        current.apply_edits(&base, unrelated);
        assert!(
            !current.preview_follow_edits,
            "an unchanged stale toggle must not overwrite it"
        );
    }

    #[test]
    fn tex_preview_follow_edits_defaults_off_persists_and_merges_independently() {
        let base = AppSettings::default();
        assert!(!base.tex_preview_follow_edits);
        assert!(base.preview_follow_edits);
        let mut edited = base.clone();
        edited.tex_preview_follow_edits = true;
        let mut storage = MemoryStorage::default();
        edited.save(&mut storage);
        let loaded = AppSettings::load(Some(&storage)).unwrap();
        assert!(loaded.tex_preview_follow_edits);
        assert!(loaded.preview_follow_edits);

        let mut value = serde_json::to_value(&edited).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("tex_preview_follow_edits");
        storage.set_string(STORAGE_KEY, value.to_string());
        let loaded = AppSettings::load(Some(&storage)).unwrap();
        assert!(!loaded.tex_preview_follow_edits);
        assert!(loaded.preview_follow_edits);

        let mut current = base.clone();
        current.preview_follow_edits = false;
        current.apply_edits(&base, edited);
        assert!(current.tex_preview_follow_edits);
        assert!(!current.preview_follow_edits);
        let mut unrelated = base.clone();
        unrelated.auto_save = false;
        current.apply_edits(&base, unrelated);
        assert!(current.tex_preview_follow_edits);
        assert!(!current.preview_follow_edits);
        assert!(!current.auto_save);
    }

    #[test]
    fn overleaf_history_is_canonical_bounded_unique_and_independently_merged() {
        let mut settings = AppSettings::default();
        assert!(settings.overleaf_projects.is_empty());
        for index in 0..12 {
            settings
                .remember_overleaf_project(&format!(
                    "https://www.overleaf.com/project/{index:024x}/"
                ))
                .unwrap();
        }
        assert_eq!(settings.overleaf_projects.len(), 8);
        assert!(settings.overleaf_projects[0].ends_with("00000000000000000000000b"));
        settings
            .remember_overleaf_project("https://WWW.OVERLEAF.COM/project/00000000000000000000000A/")
            .unwrap();
        assert_eq!(settings.overleaf_projects.len(), 8);
        assert_eq!(
            settings.overleaf_projects[0],
            "https://www.overleaf.com/project/00000000000000000000000a"
        );
        let mut storage = MemoryStorage::default();
        settings.save(&mut storage);
        assert_eq!(
            AppSettings::load(Some(&storage)).unwrap().overleaf_projects,
            settings.overleaf_projects
        );
        let base = AppSettings::default();
        let mut current = base.clone();
        current.auto_save = false;
        current.apply_edits(&base, settings.clone());
        assert!(!current.auto_save);
        assert_eq!(current.overleaf_projects, settings.overleaf_projects);
        let mut unrelated = base.clone();
        unrelated.line_wrap = false;
        current.apply_edits(&base, unrelated);
        assert_eq!(current.overleaf_projects, settings.overleaf_projects);
    }

    #[test]
    fn overleaf_history_json_rejects_invalid_and_credential_bearing_urls_with_field_errors() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        value.as_object_mut().unwrap().remove("overleaf_projects");
        assert!(
            crate::settings_json::parse(&value.to_string())
                .unwrap()
                .overleaf_projects
                .is_empty()
        );
        for invalid in [
            "https://www.overleaf.com/project/no",
            "http://www.overleaf.com/project/0123456789abcdef01234567",
            "https://user:secret@www.overleaf.com/project/0123456789abcdef01234567",
            "https://www.overleaf.com/project/0123456789abcdef01234567?token=secret",
        ] {
            value["overleaf_projects"] = serde_json::json!([invalid]);
            let error = crate::settings_json::parse(&value.to_string()).unwrap_err();
            assert!(
                error.contains("overleaf_projects") && error.contains("Overleaf project 1"),
                "{error}"
            );
            let mut storage = MemoryStorage::default();
            storage.set_string(STORAGE_KEY, value.to_string());
            assert!(AppSettings::load(Some(&storage)).is_err());
        }
        let mut projects: Vec<String> = (0..10)
            .map(|index| format!("https://overleaf.example/project/{index:024x}"))
            .collect();
        projects.insert(1, projects[0].clone());
        value["overleaf_projects"] = serde_json::json!(projects);
        let parsed = crate::settings_json::parse(&value.to_string()).unwrap();
        assert_eq!(parsed.overleaf_projects.len(), 8);
        projects.push("not a URL".into());
        value["overleaf_projects"] = serde_json::json!(projects);
        assert!(
            crate::settings_json::parse(&value.to_string()).is_err(),
            "even a truncated invalid history entry must be explained"
        );
    }
}
