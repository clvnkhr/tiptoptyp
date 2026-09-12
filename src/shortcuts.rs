//! Validated, user-configurable application keyboard shortcuts.
//!
//! Persisted settings contain only overrides keyed by stable action IDs.  An
//! effective binding set overlays those values on platform defaults and owns
//! collision resolution, so egui routing and native menus can consume the
//! same result.

use std::{collections::BTreeMap, fmt};

use eframe::egui::{self, KeyboardShortcut, Modifiers};
use serde::{Deserialize, Deserializer, Serialize};

/// Stable persisted identities for every payload-free application command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ShortcutAction {
    Settings,
    New,
    NewWindow,
    Open,
    OpenInNewWindow,
    ChangeWorkspaceRoot,
    Save,
    SaveAs,
    ExportPdf,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    ToggleComment,
    Find,
    FindReplace,
    Format,
    SyncPreview,
    Problems,
    Explorer,
    Code,
    Split,
    Preview,
    Compile,
    ToggleCompilation,
    Complete,
    FocusTooltip,
    UiScaleIn,
    UiScaleOut,
    PreviewZoomIn,
    PreviewZoomOut,
    PreviewZoomReset,
    Minimize,
    ToggleFullscreen,
    CloseWindow,
    CaptureUi,
}

impl ShortcutAction {
    pub(crate) const ALL: [Self; 38] = [
        Self::Settings,
        Self::New,
        Self::NewWindow,
        Self::Open,
        Self::OpenInNewWindow,
        Self::ChangeWorkspaceRoot,
        Self::Save,
        Self::SaveAs,
        Self::ExportPdf,
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
        Self::ToggleComment,
        Self::Find,
        Self::FindReplace,
        Self::Format,
        Self::SyncPreview,
        Self::Problems,
        Self::Explorer,
        Self::Code,
        Self::Split,
        Self::Preview,
        Self::Compile,
        Self::ToggleCompilation,
        Self::Complete,
        Self::FocusTooltip,
        Self::UiScaleIn,
        Self::UiScaleOut,
        Self::PreviewZoomIn,
        Self::PreviewZoomOut,
        Self::PreviewZoomReset,
        Self::Minimize,
        Self::ToggleFullscreen,
        Self::CloseWindow,
        Self::CaptureUi,
    ];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Settings => "application.settings",
            Self::New => "file.new",
            Self::NewWindow => "file.new_window",
            Self::Open => "file.open",
            Self::OpenInNewWindow => "file.open_in_new_window",
            Self::ChangeWorkspaceRoot => "file.change_workspace_root",
            Self::Save => "file.save",
            Self::SaveAs => "file.save_as",
            Self::ExportPdf => "file.export_pdf",
            Self::Undo => "edit.undo",
            Self::Redo => "edit.redo",
            Self::Cut => "edit.cut",
            Self::Copy => "edit.copy",
            Self::Paste => "edit.paste",
            Self::SelectAll => "edit.select_all",
            Self::ToggleComment => "edit.toggle_comment",
            Self::Find => "edit.find",
            Self::FindReplace => "edit.find_replace",
            Self::Format => "edit.format",
            Self::SyncPreview => "edit.sync_preview",
            Self::Problems => "view.problems",
            Self::Explorer => "view.explorer",
            Self::Code => "view.code",
            Self::Split => "view.split",
            Self::Preview => "view.preview",
            Self::Compile => "build.compile",
            Self::ToggleCompilation => "build.toggle_automatic_compilation",
            Self::Complete => "editor.complete",
            Self::FocusTooltip => "editor.focus_tooltip",
            Self::UiScaleIn => "window.interface_scale_in",
            Self::UiScaleOut => "window.interface_scale_out",
            Self::PreviewZoomIn => "preview.zoom_in",
            Self::PreviewZoomOut => "preview.zoom_out",
            Self::PreviewZoomReset => "preview.zoom_reset",
            Self::Minimize => "window.minimize",
            Self::ToggleFullscreen => "window.toggle_fullscreen",
            Self::CloseWindow => "window.close",
            Self::CaptureUi => "developer.capture_ui",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::New => "New document",
            Self::NewWindow => "New window",
            Self::Open => "Open",
            Self::OpenInNewWindow => "Open in new window",
            Self::ChangeWorkspaceRoot => "Change workspace root",
            Self::Save => "Save",
            Self::SaveAs => "Save as",
            Self::ExportPdf => "Export PDF",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Cut => "Cut",
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select all",
            Self::ToggleComment => "Toggle comment",
            Self::Find => "Find",
            Self::FindReplace => "Find and replace",
            Self::Format => "Format document",
            Self::SyncPreview => "Reveal in preview",
            Self::Problems => "Problems",
            Self::Explorer => "Explorer",
            Self::Code => "Code view",
            Self::Split => "Split view",
            Self::Preview => "Preview view",
            Self::Compile => "Compile PDF",
            Self::ToggleCompilation => "Pause or resume automatic preview updates",
            Self::Complete => "Show completions",
            Self::FocusTooltip => "Focus hover card",
            Self::UiScaleIn => "Increase interface scale",
            Self::UiScaleOut => "Decrease interface scale",
            Self::PreviewZoomIn => "Zoom preview in",
            Self::PreviewZoomOut => "Zoom preview out",
            Self::PreviewZoomReset => "Reset preview zoom",
            Self::Minimize => "Minimize window",
            Self::ToggleFullscreen => "Toggle full screen",
            Self::CloseWindow => "Close window",
            Self::CaptureUi => "Capture application UI",
        }
    }

    pub(crate) const fn group(self) -> &'static str {
        match self {
            Self::Settings => "Application",
            Self::New
            | Self::NewWindow
            | Self::Open
            | Self::OpenInNewWindow
            | Self::ChangeWorkspaceRoot
            | Self::Save
            | Self::SaveAs
            | Self::ExportPdf => "File",
            Self::Undo
            | Self::Redo
            | Self::Cut
            | Self::Copy
            | Self::Paste
            | Self::SelectAll
            | Self::ToggleComment
            | Self::Find
            | Self::FindReplace
            | Self::Format
            | Self::Complete => "Editor",
            Self::SyncPreview
            | Self::PreviewZoomIn
            | Self::PreviewZoomOut
            | Self::PreviewZoomReset => "Preview",
            Self::Compile | Self::ToggleCompilation => "Build",
            Self::Problems
            | Self::Explorer
            | Self::Code
            | Self::Split
            | Self::Preview
            | Self::FocusTooltip
            | Self::UiScaleIn
            | Self::UiScaleOut
            | Self::Minimize
            | Self::ToggleFullscreen
            | Self::CloseWindow => "Window",
            Self::CaptureUi => "Developer",
        }
    }
}

/// The shortcut convention to use when resolving defaults and display names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutPlatform {
    MacOs,
    Other,
}

impl ShortcutPlatform {
    pub(crate) const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

/// A platform-independent chord. `primary` means Command on macOS and Control
/// elsewhere; `control` always means the physical Control modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ShortcutChord {
    key: egui::Key,
    primary: bool,
    control: bool,
    shift: bool,
    alt: bool,
}

impl ShortcutChord {
    pub(crate) const fn primary(key: egui::Key) -> Self {
        Self {
            key,
            primary: true,
            control: false,
            shift: false,
            alt: false,
        }
    }

    const fn control(key: egui::Key) -> Self {
        Self {
            key,
            primary: false,
            control: true,
            shift: false,
            alt: false,
        }
    }

    const fn shift(mut self) -> Self {
        self.shift = true;
        self
    }

    const fn alt(mut self) -> Self {
        self.alt = true;
        self
    }

    pub(crate) fn parse(value: &str) -> Result<Self, ShortcutParseError> {
        let mut primary = false;
        let mut control = false;
        let mut shift = false;
        let mut alt = false;
        let mut key = None;

        for raw_part in value.split('+') {
            let part = raw_part.trim();
            if part.is_empty() {
                return Err(ShortcutParseError::EmptyPart);
            }
            if part.eq_ignore_ascii_case("primary")
                || part.eq_ignore_ascii_case("command")
                || part.eq_ignore_ascii_case("cmd")
            {
                set_modifier(&mut primary, "Primary")?;
            } else if part.eq_ignore_ascii_case("control") || part.eq_ignore_ascii_case("ctrl") {
                set_modifier(&mut control, "Control")?;
            } else if part.eq_ignore_ascii_case("shift") {
                set_modifier(&mut shift, "Shift")?;
            } else if part.eq_ignore_ascii_case("alt") || part.eq_ignore_ascii_case("option") {
                set_modifier(&mut alt, "Alt")?;
            } else {
                let parsed = parse_key(part)
                    .filter(|key| supported_key(*key))
                    .ok_or_else(|| ShortcutParseError::UnsupportedKey(part.to_owned()))?;
                if key.replace(parsed).is_some() {
                    return Err(ShortcutParseError::MultipleKeys);
                }
            }
        }

        let key = key.ok_or(ShortcutParseError::MissingKey)?;
        validate_modifier_requirement(key, primary, control, alt)?;
        Ok(Self {
            key,
            primary,
            control,
            shift,
            alt,
        })
    }

    /// Convert a captured egui chord into the portable representation used by
    /// settings. On non-macOS platforms egui marks Control as both the physical
    /// Control key and the logical primary command modifier.
    pub(crate) fn from_egui(
        shortcut: KeyboardShortcut,
        platform: ShortcutPlatform,
    ) -> Result<Self, ShortcutParseError> {
        if !supported_key(shortcut.logical_key) {
            return Err(ShortcutParseError::UnsupportedKey(
                shortcut.logical_key.name().to_owned(),
            ));
        }
        let primary = shortcut.modifiers.command;
        let control = shortcut.modifiers.ctrl
            && (platform == ShortcutPlatform::MacOs || !shortcut.modifiers.command);
        validate_modifier_requirement(
            shortcut.logical_key,
            primary,
            control,
            shortcut.modifiers.alt,
        )?;
        Ok(Self {
            key: shortcut.logical_key,
            primary,
            control,
            shift: shortcut.modifiers.shift,
            alt: shortcut.modifiers.alt,
        })
    }

    #[cfg(test)]
    pub(crate) const fn key(self) -> egui::Key {
        self.key
    }

    pub(crate) const fn primary_modifier(self) -> bool {
        self.primary
    }

    pub(crate) const fn control_modifier(self) -> bool {
        self.control
    }

    pub(crate) const fn shift_modifier(self) -> bool {
        self.shift
    }

    pub(crate) const fn alt_modifier(self) -> bool {
        self.alt
    }

    pub(crate) fn egui(self) -> KeyboardShortcut {
        let mut modifiers = Modifiers::NONE;
        if self.primary {
            modifiers |= Modifiers::COMMAND;
        }
        if self.control {
            modifiers |= Modifiers::CTRL;
        }
        if self.shift {
            modifiers |= Modifiers::SHIFT;
        }
        if self.alt {
            modifiers |= Modifiers::ALT;
        }
        KeyboardShortcut::new(modifiers, self.key)
    }

    pub(crate) fn specificity(self) -> u8 {
        u8::from(self.primary) + u8::from(self.control) + u8::from(self.shift) + u8::from(self.alt)
    }

    /// Canonical, platform-independent form used in persisted settings.
    pub(crate) fn config_string(self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.primary {
            parts.push("Primary");
        }
        if self.control {
            parts.push("Control");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(config_key_name(self.key));
        parts.join("+")
    }

    /// Human-readable form for settings controls outside an egui context.
    pub(crate) fn display(self, platform: ShortcutPlatform) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.primary {
            parts.push(match platform {
                ShortcutPlatform::MacOs => "Cmd",
                ShortcutPlatform::Other => "Ctrl",
            });
        }
        if self.control && !(self.primary && platform == ShortcutPlatform::Other) {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push(match platform {
                ShortcutPlatform::MacOs => "Option",
                ShortcutPlatform::Other => "Alt",
            });
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(display_key_name(self.key));
        parts.join("+")
    }

    pub(crate) fn appkit_key_equivalent(self) -> String {
        let key = self.key;
        if (egui::Key::A..=egui::Key::Z).contains(&key) {
            return key.name().to_ascii_lowercase();
        }
        if (egui::Key::Num0..=egui::Key::Num9).contains(&key) {
            return key.name().to_owned();
        }
        if (egui::Key::F1..=egui::Key::F35).contains(&key) {
            let number = key.name()[1..]
                .parse::<u32>()
                .expect("egui function-key names contain a number");
            return char::from_u32(0xF704 + number - 1)
                .expect("AppKit function-key code points are valid")
                .to_string();
        }
        match key {
            egui::Key::ArrowUp => "\u{F700}",
            egui::Key::ArrowDown => "\u{F701}",
            egui::Key::ArrowLeft => "\u{F702}",
            egui::Key::ArrowRight => "\u{F703}",
            egui::Key::Escape => "\u{1b}",
            egui::Key::Tab => "\t",
            egui::Key::Backspace => "\u{8}",
            egui::Key::Enter => "\r",
            egui::Key::Space => " ",
            egui::Key::Insert => "\u{F727}",
            egui::Key::Delete => "\u{F728}",
            egui::Key::Home => "\u{F729}",
            egui::Key::End => "\u{F72B}",
            egui::Key::PageUp => "\u{F72C}",
            egui::Key::PageDown => "\u{F72D}",
            egui::Key::Comma => ",",
            egui::Key::Backslash => "\\",
            egui::Key::Slash => "/",
            egui::Key::OpenBracket => "[",
            egui::Key::CloseBracket => "]",
            egui::Key::Backtick => "`",
            egui::Key::Plus => "+",
            egui::Key::Minus => "-",
            egui::Key::Period => ".",
            egui::Key::Equals => "=",
            egui::Key::Semicolon => ";",
            egui::Key::Quote => "'",
            _ => unreachable!("shortcut parsing rejects keys without AppKit equivalents"),
        }
        .to_owned()
    }

    fn collision_key(self, platform: ShortcutPlatform) -> PhysicalChord {
        PhysicalChord {
            key: self.key,
            command: self.primary && platform == ShortcutPlatform::MacOs,
            control: self.control || (self.primary && platform == ShortcutPlatform::Other),
            shift: self.shift,
            alt: self.alt,
        }
    }
}

fn validate_modifier_requirement(
    key: egui::Key,
    primary: bool,
    control: bool,
    alt: bool,
) -> Result<(), ShortcutParseError> {
    if !primary && !control && !alt && !(egui::Key::F1..=egui::Key::F35).contains(&key) {
        Err(ShortcutParseError::ModifierRequired)
    } else {
        Ok(())
    }
}

fn set_modifier(slot: &mut bool, name: &'static str) -> Result<(), ShortcutParseError> {
    if std::mem::replace(slot, true) {
        Err(ShortcutParseError::DuplicateModifier(name))
    } else {
        Ok(())
    }
}

fn parse_key(value: &str) -> Option<egui::Key> {
    egui::Key::from_name(value).or_else(|| {
        egui::Key::ALL
            .iter()
            .copied()
            .find(|key| key.name().eq_ignore_ascii_case(value))
    })
}

fn supported_key(key: egui::Key) -> bool {
    (egui::Key::A..=egui::Key::Z).contains(&key)
        || (egui::Key::Num0..=egui::Key::Num9).contains(&key)
        || (egui::Key::F1..=egui::Key::F35).contains(&key)
        || matches!(
            key,
            egui::Key::ArrowDown
                | egui::Key::ArrowLeft
                | egui::Key::ArrowRight
                | egui::Key::ArrowUp
                | egui::Key::Escape
                | egui::Key::Tab
                | egui::Key::Backspace
                | egui::Key::Enter
                | egui::Key::Space
                | egui::Key::Insert
                | egui::Key::Delete
                | egui::Key::Home
                | egui::Key::End
                | egui::Key::PageUp
                | egui::Key::PageDown
                | egui::Key::Comma
                | egui::Key::Backslash
                | egui::Key::Slash
                | egui::Key::OpenBracket
                | egui::Key::CloseBracket
                | egui::Key::Backtick
                | egui::Key::Plus
                | egui::Key::Minus
                | egui::Key::Period
                | egui::Key::Equals
                | egui::Key::Semicolon
                | egui::Key::Quote
        )
}

fn config_key_name(key: egui::Key) -> &'static str {
    key.name()
}

fn display_key_name(key: egui::Key) -> &'static str {
    match key {
        egui::Key::Comma => ",",
        egui::Key::Backslash => "\\",
        egui::Key::Slash => "/",
        egui::Key::OpenBracket => "[",
        egui::Key::CloseBracket => "]",
        egui::Key::Backtick => "`",
        egui::Key::Minus => "-",
        egui::Key::Period => ".",
        egui::Key::Equals => "=",
        egui::Key::Semicolon => ";",
        egui::Key::Quote => "'",
        _ => key.name(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShortcutParseError {
    EmptyPart,
    MissingKey,
    MultipleKeys,
    DuplicateModifier(&'static str),
    UnsupportedKey(String),
    ModifierRequired,
}

impl fmt::Display for ShortcutParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPart => formatter.write_str("shortcut contains an empty component"),
            Self::MissingKey => formatter.write_str("shortcut does not contain a key"),
            Self::MultipleKeys => formatter.write_str("shortcut contains more than one key"),
            Self::DuplicateModifier(modifier) => {
                write!(formatter, "shortcut repeats the {modifier} modifier")
            }
            Self::UnsupportedKey(key) => write!(formatter, "unsupported shortcut key {key:?}"),
            Self::ModifierRequired => formatter.write_str(
                "shortcuts need Command, Control, or Alt unless they use a function key",
            ),
        }
    }
}

impl std::error::Error for ShortcutParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PhysicalChord {
    key: egui::Key,
    command: bool,
    control: bool,
    shift: bool,
    alt: bool,
}

/// Raw persisted overrides. Invalid or unknown entries are discarded by
/// [`Self::normalize`] without invalidating unrelated application settings.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct ShortcutOverrides {
    entries: BTreeMap<String, Option<String>>,
}

impl<'de> Deserialize<'de> for ShortcutOverrides {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = BTreeMap::<String, serde_json::Value>::deserialize(deserializer)?;
        let entries = raw
            .into_iter()
            .filter_map(|(id, value)| match value {
                serde_json::Value::Null => Some((id, None)),
                serde_json::Value::String(value) => Some((id, Some(value))),
                _ => None,
            })
            .collect();
        Ok(Self { entries })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShortcutNormalization {
    pub(crate) unknown_actions: usize,
    pub(crate) invalid_bindings: usize,
}

impl ShortcutOverrides {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `None` means the action follows its platform default; `Some(None)` is
    /// an explicit disabled binding.
    pub(crate) fn get(&self, action: ShortcutAction) -> Option<Option<ShortcutChord>> {
        let value = self.entries.get(action.id())?;
        match value {
            None => Some(None),
            Some(value) => ShortcutChord::parse(value).ok().map(Some),
        }
    }

    /// Canonicalize valid IDs/chords and discard corrupt entries.
    pub(crate) fn normalize(&mut self) -> ShortcutNormalization {
        let mut report = ShortcutNormalization::default();
        let raw = std::mem::take(&mut self.entries);
        for (id, value) in raw {
            let Some(action) = ShortcutAction::from_id(&id) else {
                report.unknown_actions += 1;
                continue;
            };
            let value = match value {
                None => None,
                Some(value) => match ShortcutChord::parse(&value) {
                    Ok(chord) => Some(chord.config_string()),
                    Err(_) => {
                        report.invalid_bindings += 1;
                        continue;
                    }
                },
            };
            self.entries.insert(action.id().to_owned(), value);
        }
        report
    }

    pub(crate) fn set(&mut self, action: ShortcutAction, chord: Option<ShortcutChord>) {
        self.entries.insert(
            action.id().to_owned(),
            chord.map(ShortcutChord::config_string),
        );
    }

    /// Assign a chord and deterministically disable the currently effective
    /// action it displaces. The newly assigned action always wins.
    pub(crate) fn assign(
        &mut self,
        action: ShortcutAction,
        chord: ShortcutChord,
        platform: ShortcutPlatform,
    ) -> Option<ShortcutAction> {
        let physical = chord.collision_key(platform);
        let displaced = ShortcutBindings::from_overrides(platform, self)
            .iter()
            .find_map(|(candidate, binding)| {
                (candidate != action
                    && binding.is_some_and(|binding| binding.collision_key(platform) == physical))
                .then_some(candidate)
            });
        // Persisted settings may contain several explicit owners for the same
        // physical chord. Only one is effective, but leaving the hidden owners
        // intact could make one of them beat this new assignment after the
        // visible winner is disabled. Clear every owner so the action selected
        // by the user is guaranteed to win.
        let colliding = ShortcutAction::ALL
            .into_iter()
            .filter(|&candidate| candidate != action)
            .filter(|&candidate| {
                let binding = match self.get(candidate) {
                    Some(binding) => binding,
                    None => default_binding(candidate, platform),
                };
                binding.is_some_and(|binding| binding.collision_key(platform) == physical)
            })
            .collect::<Vec<_>>();
        for candidate in colliding {
            self.set(candidate, None);
        }
        self.set(action, Some(chord));
        displaced
    }

    pub(crate) fn reset(&mut self, action: ShortcutAction) {
        self.entries.remove(action.id());
    }

    pub(crate) fn reset_all(&mut self) {
        self.entries.clear();
    }

    fn parsed(&self) -> BTreeMap<ShortcutAction, Option<ShortcutChord>> {
        let mut parsed = BTreeMap::new();
        for (id, value) in &self.entries {
            let (Some(action), value) = (
                ShortcutAction::from_id(id),
                value.as_deref().map(ShortcutChord::parse).transpose().ok(),
            ) else {
                continue;
            };
            if let Some(value) = value {
                parsed.insert(action, value);
            }
        }
        parsed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShortcutConflict {
    pub(crate) winner: ShortcutAction,
    pub(crate) unbound: ShortcutAction,
    pub(crate) chord: ShortcutChord,
}

/// Complete, validated bindings for one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShortcutBindings {
    platform: ShortcutPlatform,
    bindings: BTreeMap<ShortcutAction, Option<ShortcutChord>>,
    conflicts: Vec<ShortcutConflict>,
}

impl ShortcutBindings {
    pub(crate) fn current_defaults() -> Self {
        Self::defaults(ShortcutPlatform::current())
    }

    pub(crate) fn defaults(platform: ShortcutPlatform) -> Self {
        Self::from_overrides(platform, &ShortcutOverrides::default())
    }

    pub(crate) fn current(overrides: &ShortcutOverrides) -> Self {
        Self::from_overrides(ShortcutPlatform::current(), overrides)
    }

    pub(crate) fn from_overrides(
        platform: ShortcutPlatform,
        overrides: &ShortcutOverrides,
    ) -> Self {
        let overrides = overrides.parsed();
        let mut bindings = BTreeMap::new();
        let mut claimed = BTreeMap::<PhysicalChord, ShortcutAction>::new();
        let mut conflicts = Vec::new();

        // Explicit choices claim chords before defaults. Conflicts between two
        // persisted choices are resolved by stable `ShortcutAction::ALL` order.
        for action in ShortcutAction::ALL {
            let Some(binding) = overrides.get(&action) else {
                continue;
            };
            let accepted = binding.and_then(|chord| {
                let physical = chord.collision_key(platform);
                if let Some(&winner) = claimed.get(&physical) {
                    conflicts.push(ShortcutConflict {
                        winner,
                        unbound: action,
                        chord,
                    });
                    None
                } else {
                    claimed.insert(physical, action);
                    Some(chord)
                }
            });
            bindings.insert(action, accepted);
        }

        for action in ShortcutAction::ALL {
            if overrides.contains_key(&action) {
                continue;
            }
            let Some(chord) = default_binding(action, platform) else {
                bindings.insert(action, None);
                continue;
            };
            let physical = chord.collision_key(platform);
            if let Some(&winner) = claimed.get(&physical) {
                conflicts.push(ShortcutConflict {
                    winner,
                    unbound: action,
                    chord,
                });
                bindings.insert(action, None);
            } else {
                claimed.insert(physical, action);
                bindings.insert(action, Some(chord));
            }
        }

        debug_assert_eq!(bindings.len(), ShortcutAction::ALL.len());
        Self {
            platform,
            bindings,
            conflicts,
        }
    }

    pub(crate) fn binding(&self, action: ShortcutAction) -> Option<ShortcutChord> {
        self.bindings.get(&action).copied().flatten()
    }

    pub(crate) fn egui(&self, action: ShortcutAction) -> Option<KeyboardShortcut> {
        self.binding(action).map(ShortcutChord::egui)
    }

    /// Resolve the action which owns a concrete key press using the same
    /// most-specific-first rule as the runtime shortcut router. This is also
    /// used to recover Copy/Cut/Paste key presses that egui-winit translates
    /// into semantic events before application routing sees them.
    pub(crate) fn action_for_key_event(
        &self,
        key: egui::Key,
        modifiers: Modifiers,
    ) -> Option<ShortcutAction> {
        for specificity in (0..=4).rev() {
            for action in ShortcutAction::ALL {
                let Some(chord) = self.binding(action) else {
                    continue;
                };
                if chord.key == key
                    && chord.specificity() == specificity
                    && modifiers.matches_logically(chord.egui().modifiers)
                {
                    return Some(action);
                }
            }
        }
        None
    }

    pub(crate) fn display(&self, action: ShortcutAction) -> Option<String> {
        self.binding(action)
            .map(|binding| binding.display(self.platform))
    }

    pub(crate) fn iter(
        &self,
    ) -> impl Iterator<Item = (ShortcutAction, Option<ShortcutChord>)> + '_ {
        ShortcutAction::ALL
            .into_iter()
            .map(|action| (action, self.binding(action)))
    }

    pub(crate) fn conflicts(&self) -> &[ShortcutConflict] {
        &self.conflicts
    }
}

/// Consume the most-specific effective shortcut across every application
/// action. Callers can restrict the actions that are meaningful at a given
/// point while a final unrestricted pass prevents an application binding
/// from leaking through to a focused widget as a different built-in command.
pub(crate) fn consume_shortcut_action(
    input: &mut egui::InputState,
    bindings: &ShortcutBindings,
    accepts: impl Fn(ShortcutAction) -> bool,
) -> Option<ShortcutAction> {
    for specificity in (0..=4).rev() {
        for action in ShortcutAction::ALL {
            let Some(chord) = bindings.binding(action) else {
                continue;
            };
            if chord.specificity() == specificity
                && accepts(action)
                && input.consume_shortcut(&chord.egui())
            {
                return Some(action);
            }
        }
    }
    None
}

fn default_binding(action: ShortcutAction, platform: ShortcutPlatform) -> Option<ShortcutChord> {
    use ShortcutAction as Action;
    use egui::Key;

    Some(match action {
        Action::Settings => ShortcutChord::primary(Key::Comma),
        Action::New => ShortcutChord::primary(Key::N),
        Action::NewWindow => ShortcutChord::primary(Key::N).shift(),
        Action::Open => ShortcutChord::primary(Key::O),
        Action::OpenInNewWindow => ShortcutChord::primary(Key::O).alt(),
        Action::ChangeWorkspaceRoot => ShortcutChord::primary(Key::O).shift(),
        Action::Save => ShortcutChord::primary(Key::S),
        Action::SaveAs => ShortcutChord::primary(Key::S).shift(),
        Action::ExportPdf => ShortcutChord::primary(Key::E).shift(),
        Action::Undo => ShortcutChord::primary(Key::Z),
        Action::Redo => ShortcutChord::primary(Key::Z).shift(),
        Action::Cut => ShortcutChord::primary(Key::X),
        Action::Copy => ShortcutChord::primary(Key::C),
        Action::Paste => ShortcutChord::primary(Key::V),
        Action::SelectAll => ShortcutChord::primary(Key::A),
        Action::ToggleComment => ShortcutChord::primary(Key::Slash),
        Action::Find => ShortcutChord::primary(Key::F),
        Action::FindReplace if platform == ShortcutPlatform::MacOs => {
            ShortcutChord::primary(Key::F).alt()
        }
        Action::FindReplace => ShortcutChord::control(Key::H),
        Action::Format => ShortcutChord {
            key: Key::F,
            primary: false,
            control: false,
            shift: true,
            alt: true,
        },
        Action::SyncPreview => return None,
        Action::Problems => ShortcutChord::primary(Key::Num5),
        Action::Explorer => ShortcutChord::primary(Key::Num1),
        Action::Code => ShortcutChord::primary(Key::Num2),
        Action::Split => ShortcutChord::primary(Key::Num3),
        Action::Preview => ShortcutChord::primary(Key::Num4),
        Action::Compile => ShortcutChord::primary(Key::R),
        Action::ToggleCompilation => ShortcutChord::primary(Key::R).shift(),
        Action::Complete => ShortcutChord::control(Key::Space),
        Action::FocusTooltip => ShortcutChord::primary(Key::Space).shift(),
        Action::UiScaleIn => ShortcutChord::primary(Key::Plus),
        Action::UiScaleOut => ShortcutChord::primary(Key::Minus),
        Action::PreviewZoomIn => ShortcutChord::primary(Key::Plus).alt(),
        Action::PreviewZoomOut => ShortcutChord::primary(Key::Minus).alt(),
        Action::PreviewZoomReset => ShortcutChord::primary(Key::Num0).alt(),
        Action::Minimize => ShortcutChord::primary(Key::M),
        Action::ToggleFullscreen if platform == ShortcutPlatform::MacOs => ShortcutChord {
            key: Key::F,
            primary: true,
            control: true,
            shift: false,
            alt: false,
        },
        Action::ToggleFullscreen => return None,
        Action::CloseWindow => ShortcutChord::primary(Key::W),
        Action::CaptureUi => ShortcutChord::primary(Key::F12).shift(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn action_ids_are_stable_unique_and_round_trip() {
        let ids = ShortcutAction::ALL
            .into_iter()
            .map(ShortcutAction::id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), ShortcutAction::ALL.len());
        for action in ShortcutAction::ALL {
            assert_eq!(ShortcutAction::from_id(action.id()), Some(action));
        }
        assert_eq!(ShortcutAction::from_id("file.open_recent"), None);
    }

    #[test]
    fn defaults_cover_every_action_and_have_no_platform_collisions() {
        for platform in [ShortcutPlatform::MacOs, ShortcutPlatform::Other] {
            let bindings = ShortcutBindings::defaults(platform);
            assert_eq!(bindings.iter().count(), ShortcutAction::ALL.len());
            assert!(bindings.conflicts().is_empty());
            assert_eq!(bindings.binding(ShortcutAction::SyncPreview), None);
            assert!(ShortcutAction::ALL.into_iter().all(|action| {
                matches!(
                    action,
                    ShortcutAction::SyncPreview | ShortcutAction::ToggleFullscreen
                ) && platform == ShortcutPlatform::Other
                    || action == ShortcutAction::SyncPreview
                    || bindings.binding(action).is_some()
            }));
        }

        assert_eq!(
            ShortcutBindings::defaults(ShortcutPlatform::MacOs)
                .display(ShortcutAction::FindReplace)
                .as_deref(),
            Some("Cmd+Option+F")
        );
        assert_eq!(
            ShortcutBindings::defaults(ShortcutPlatform::Other)
                .display(ShortcutAction::FindReplace)
                .as_deref(),
            Some("Ctrl+H")
        );
    }

    #[test]
    fn parser_canonicalizes_aliases_and_rejects_ambiguous_input() {
        let chord = ShortcutChord::parse("cmd + option + shift + /").unwrap();
        assert_eq!(chord.config_string(), "Primary+Alt+Shift+Slash");
        assert_eq!(chord.display(ShortcutPlatform::MacOs), "Cmd+Option+Shift+/");
        assert_eq!(chord.display(ShortcutPlatform::Other), "Ctrl+Alt+Shift+/");
        assert_eq!(
            ShortcutChord::parse("Ctrl+Ctrl+F"),
            Err(ShortcutParseError::DuplicateModifier("Control"))
        );
        assert_eq!(
            ShortcutChord::parse("Primary+F+G"),
            Err(ShortcutParseError::MultipleKeys)
        );
        assert_eq!(
            ShortcutChord::parse("Primary"),
            Err(ShortcutParseError::MissingKey)
        );
        assert!(matches!(
            ShortcutChord::parse("Primary+BrowserBack"),
            Err(ShortcutParseError::UnsupportedKey(_))
        ));

        let captured = KeyboardShortcut::new(
            Modifiers {
                ctrl: true,
                command: true,
                shift: true,
                ..Modifiers::NONE
            },
            egui::Key::K,
        );
        assert_eq!(
            ShortcutChord::from_egui(captured, ShortcutPlatform::Other)
                .unwrap()
                .config_string(),
            "Primary+Shift+K"
        );
    }

    #[test]
    fn capture_requires_a_modifier_for_printable_keys() {
        assert_eq!(
            ShortcutChord::parse("A"),
            Err(ShortcutParseError::ModifierRequired)
        );
        assert_eq!(
            ShortcutChord::parse("Shift+A"),
            Err(ShortcutParseError::ModifierRequired)
        );
        assert_eq!(
            ShortcutChord::from_egui(
                KeyboardShortcut::new(Modifiers::NONE, egui::Key::A),
                ShortcutPlatform::MacOs,
            ),
            Err(ShortcutParseError::ModifierRequired)
        );
        assert_eq!(
            ShortcutChord::from_egui(
                KeyboardShortcut::new(Modifiers::SHIFT, egui::Key::A),
                ShortcutPlatform::MacOs,
            ),
            Err(ShortcutParseError::ModifierRequired)
        );
        assert!(
            ShortcutChord::from_egui(
                KeyboardShortcut::new(Modifiers::NONE, egui::Key::F8),
                ShortcutPlatform::MacOs,
            )
            .is_ok()
        );
    }

    #[test]
    fn malformed_override_value_does_not_discard_valid_neighbors() {
        let overrides: ShortcutOverrides = serde_json::from_value(serde_json::json!({
            "file.save": "Primary+Shift+K",
            "file.open": 42,
            "view.preview": null
        }))
        .unwrap();
        assert!(overrides.get(ShortcutAction::Open).is_none());
        assert_eq!(overrides.get(ShortcutAction::Preview), Some(None));
        assert_eq!(
            overrides
                .get(ShortcutAction::Save)
                .flatten()
                .unwrap()
                .config_string(),
            "Primary+Shift+K"
        );
    }

    #[test]
    fn persisted_overrides_normalize_without_poisoning_valid_entries() {
        let mut overrides: ShortcutOverrides = serde_json::from_str(
            r#"{
                "file.save": "cmd + shift + s",
                "edit.copy": null,
                "edit.find": "Primary+NoSuchKey",
                "future.action": "Primary+Q"
            }"#,
        )
        .unwrap();
        let report = overrides.normalize();
        assert_eq!(
            report,
            ShortcutNormalization {
                unknown_actions: 1,
                invalid_bindings: 1,
            }
        );
        assert_eq!(
            serde_json::to_value(&overrides).unwrap(),
            serde_json::json!({
                "edit.copy": null,
                "file.save": "Primary+Shift+S",
            })
        );
    }

    #[test]
    fn explicit_override_wins_over_a_conflicting_default() {
        let mut overrides = ShortcutOverrides::default();
        overrides.set(
            ShortcutAction::Save,
            Some(ShortcutChord::parse("Primary+O").unwrap()),
        );
        let bindings = ShortcutBindings::from_overrides(ShortcutPlatform::MacOs, &overrides);
        assert_eq!(
            bindings.display(ShortcutAction::Save).as_deref(),
            Some("Cmd+O")
        );
        assert_eq!(bindings.binding(ShortcutAction::Open), None);
        assert_eq!(
            bindings.conflicts(),
            [ShortcutConflict {
                winner: ShortcutAction::Save,
                unbound: ShortcutAction::Open,
                chord: ShortcutChord::parse("Primary+O").unwrap(),
            }]
        );
    }

    #[test]
    fn colliding_persisted_overrides_use_stable_action_order() {
        let mut overrides = ShortcutOverrides::default();
        let chord = ShortcutChord::parse("Primary+Q").unwrap();
        overrides.set(ShortcutAction::Preview, Some(chord));
        overrides.set(ShortcutAction::Save, Some(chord));
        let bindings = ShortcutBindings::from_overrides(ShortcutPlatform::Other, &overrides);
        assert_eq!(bindings.binding(ShortcutAction::Save), Some(chord));
        assert_eq!(bindings.binding(ShortcutAction::Preview), None);
        assert_eq!(bindings.conflicts()[0].winner, ShortcutAction::Save);
        assert_eq!(bindings.conflicts()[0].unbound, ShortcutAction::Preview);
    }

    #[test]
    fn assignment_clears_hidden_persisted_owners_so_the_new_action_wins() {
        let mut overrides = ShortcutOverrides::default();
        let chord = ShortcutChord::parse("Primary+Q").unwrap();
        overrides.set(ShortcutAction::Save, Some(chord));
        overrides.set(ShortcutAction::Preview, Some(chord));
        assert_eq!(
            ShortcutBindings::from_overrides(ShortcutPlatform::Other, &overrides)
                .binding(ShortcutAction::Save),
            Some(chord)
        );

        assert_eq!(
            overrides.assign(ShortcutAction::CaptureUi, chord, ShortcutPlatform::Other,),
            Some(ShortcutAction::Save)
        );
        let bindings = ShortcutBindings::from_overrides(ShortcutPlatform::Other, &overrides);
        assert_eq!(bindings.binding(ShortcutAction::CaptureUi), Some(chord));
        assert_eq!(bindings.binding(ShortcutAction::Save), None);
        assert_eq!(bindings.binding(ShortcutAction::Preview), None);
        assert_eq!(overrides.get(ShortcutAction::Save), Some(None));
        assert_eq!(overrides.get(ShortcutAction::Preview), Some(None));
    }

    #[test]
    fn assignment_displaces_the_old_owner_and_reset_is_explicit() {
        let mut overrides = ShortcutOverrides::default();
        let chord = ShortcutChord::parse("Primary+O").unwrap();
        assert_eq!(
            overrides.assign(ShortcutAction::Save, chord, ShortcutPlatform::MacOs),
            Some(ShortcutAction::Open)
        );
        let bindings = ShortcutBindings::from_overrides(ShortcutPlatform::MacOs, &overrides);
        assert_eq!(bindings.binding(ShortcutAction::Save), Some(chord));
        assert_eq!(bindings.binding(ShortcutAction::Open), None);
        assert_eq!(overrides.get(ShortcutAction::Save), Some(Some(chord)));
        assert_eq!(overrides.get(ShortcutAction::Open), Some(None));

        overrides.reset(ShortcutAction::Save);
        assert_eq!(overrides.get(ShortcutAction::Save), None);
        assert_eq!(
            ShortcutBindings::from_overrides(ShortcutPlatform::MacOs, &overrides)
                .binding(ShortcutAction::Save),
            Some(ShortcutChord::parse("Primary+S").unwrap())
        );
        assert_eq!(
            ShortcutBindings::from_overrides(ShortcutPlatform::MacOs, &overrides)
                .binding(ShortcutAction::Open),
            None
        );
        overrides.reset_all();
        assert!(overrides.is_empty());
        assert_eq!(
            ShortcutBindings::from_overrides(ShortcutPlatform::MacOs, &overrides)
                .binding(ShortcutAction::Open),
            Some(chord)
        );
    }

    #[test]
    fn concrete_key_resolution_uses_the_most_specific_effective_owner() {
        let defaults = ShortcutBindings::defaults(ShortcutPlatform::MacOs);
        assert_eq!(
            defaults.action_for_key_event(egui::Key::C, Modifiers::COMMAND | Modifiers::SHIFT),
            Some(ShortcutAction::Copy)
        );

        let mut overrides = ShortcutOverrides::default();
        overrides.assign(
            ShortcutAction::Save,
            ShortcutChord::parse("Primary+Shift+C").unwrap(),
            ShortcutPlatform::MacOs,
        );
        overrides.set(ShortcutAction::Copy, None);
        let rebound = ShortcutBindings::from_overrides(ShortcutPlatform::MacOs, &overrides);
        assert_eq!(
            rebound.action_for_key_event(egui::Key::C, Modifiers::COMMAND | Modifiers::SHIFT),
            Some(ShortcutAction::Save)
        );
        assert_eq!(
            rebound.action_for_key_event(egui::Key::C, Modifiers::COMMAND),
            None
        );
    }

    #[test]
    fn appkit_equivalents_cover_printable_navigation_and_function_keys() {
        for (text, expected) in [
            ("Primary+A", "a"),
            ("Primary+1", "1"),
            ("Primary+/", "/"),
            ("Primary+Up", "\u{F700}"),
            ("Primary+F12", "\u{F70F}"),
            ("Primary+Delete", "\u{F728}"),
        ] {
            assert_eq!(
                ShortcutChord::parse(text).unwrap().appkit_key_equivalent(),
                expected
            );
        }
    }
}
