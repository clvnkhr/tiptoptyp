//! Import Sublime Text color schemes without depending on a UI toolkit.
//!
//! TextMate `.tmTheme` files are delegated to Syntect's plist loader. Modern
//! `.sublime-color-scheme` files use a small compatibility parser because
//! Syntect deliberately does not parse that format. Both paths produce the
//! same Syntect theme and a compact semantic palette for the rest of the app.

use std::{
    collections::HashMap,
    fmt, fs,
    io::{self, BufReader, Cursor},
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::Deserialize;
use serde_json::Value;
use syntect::{
    highlighting::{
        Color as SyntectColor, FontStyle, Highlighter, ScopeSelectors, StyleModifier, Theme,
        ThemeItem, ThemeSet,
    },
    parsing::ScopeStack,
};

const MIN_TEXT_CONTRAST: f32 = 4.5;
const MAX_VARIABLE_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeFormat {
    // Standalone importer tests compile this module without the app resolver.
    #[allow(dead_code)]
    Builtin,
    TextMate,
    SublimeColorScheme,
}

impl ThemeFormat {
    fn from_path(path: &Path) -> Result<Self, ImportError> {
        let extension = path.extension().and_then(|value| value.to_str());
        if extension.is_some_and(|value| value.eq_ignore_ascii_case("tmtheme")) {
            Ok(Self::TextMate)
        } else if extension.is_some_and(|value| value.eq_ignore_ascii_case("sublime-color-scheme"))
        {
            Ok(Self::SublimeColorScheme)
        } else {
            Err(ImportError::UnsupportedFormat {
                path: path.to_owned(),
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }

    /// WCAG contrast after compositing this color over `background`.
    pub fn contrast_ratio(self, background: Self) -> f32 {
        let foreground = self.composite_over(background);
        let lighter = foreground
            .relative_luminance()
            .max(background.relative_luminance());
        let darker = foreground
            .relative_luminance()
            .min(background.relative_luminance());
        (lighter + 0.05) / (darker + 0.05)
    }

    fn with_alpha(self, alpha: u8) -> Self {
        Self { a: alpha, ..self }
    }

    fn composite_over(self, background: Self) -> Self {
        if self.a == 255 {
            return Self { a: 255, ..self };
        }
        let alpha = f32::from(self.a) / 255.0;
        Self::rgb(
            mix_channel(background.r, self.r, alpha),
            mix_channel(background.g, self.g, alpha),
            mix_channel(background.b, self.b, alpha),
        )
    }

    fn mix(self, other: Self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        Self::from_rgba(
            mix_channel(self.r, other.r, amount),
            mix_channel(self.g, other.g, amount),
            mix_channel(self.b, other.b, amount),
            mix_channel(self.a, other.a, amount),
        )
    }

    fn relative_luminance(self) -> f32 {
        fn linear(channel: u8) -> f32 {
            let channel = f32::from(channel) / 255.0;
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }
}

impl From<SyntectColor> for Rgba {
    fn from(color: SyntectColor) -> Self {
        Self::from_rgba(color.r, color.g, color.b, color.a)
    }
}

impl From<Rgba> for SyntectColor {
    fn from(color: Rgba) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        }
    }
}

/// Theme roles used by both application chrome and editor rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticPalette {
    pub background: Rgba,
    pub surface: Rgba,
    pub elevated_surface: Rgba,
    pub foreground: Rgba,
    pub muted: Rgba,
    pub border: Rgba,
    pub accent: Rgba,
    pub error: Rgba,
    pub warning: Rgba,
    pub info: Rgba,
    pub success: Rgba,
    pub editor_background: Rgba,
    pub current_line: Rgba,
    pub selection: Rgba,
    pub selection_foreground: Rgba,
    pub caret: Rgba,
    pub gutter_background: Rgba,
    pub gutter_foreground: Rgba,
    pub plain: Rgba,
    pub comment: Rgba,
    pub operator: Rgba,
    pub number: Rgba,
    pub emphasis: Rgba,
    pub link: Rgba,
    pub string: Rgba,
    pub label: Rgba,
    pub heading: Rgba,
    pub keyword: Rgba,
    pub interpolated: Rgba,
    pub error_background: Rgba,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportedTheme {
    pub name: Option<String>,
    pub author: Option<String>,
    pub format: ThemeFormat,
    pub dark_mode: bool,
    pub palette: SemanticPalette,
    pub syntect_theme: Theme,
}

#[derive(Debug)]
pub enum ImportError {
    UnsupportedFormat { path: PathBuf },
    Read { path: PathBuf, source: io::Error },
    Parse { path: PathBuf, message: String },
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat { path } => write!(
                formatter,
                "unsupported Sublime theme format: {}",
                path.display()
            ),
            Self::Read { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::Parse { path, message } => {
                write!(formatter, "could not parse {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::UnsupportedFormat { .. } | Self::Parse { .. } => None,
        }
    }
}

pub fn import_path(path: &Path) -> Result<ImportedTheme, ImportError> {
    let bytes = fs::read(path).map_err(|source| ImportError::Read {
        path: path.to_owned(),
        source,
    })?;
    import_bytes(path, &bytes)
}

pub fn import_bytes(path: &Path, bytes: &[u8]) -> Result<ImportedTheme, ImportError> {
    let format = ThemeFormat::from_path(path)?;
    let syntect_theme = match format {
        ThemeFormat::Builtin => unreachable!("built-in themes are not loaded from files"),
        ThemeFormat::TextMate => parse_textmate(path, bytes)?,
        ThemeFormat::SublimeColorScheme => parse_color_scheme(path, bytes)?,
    };
    let name = syntect_theme.name.clone().or_else(|| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
    });
    let author = syntect_theme.author.clone();
    let (dark_mode, palette) = derive_palette(&syntect_theme, name.as_deref());
    Ok(ImportedTheme {
        name,
        author,
        format,
        dark_mode,
        palette,
        syntect_theme,
    })
}

fn parse_textmate(path: &Path, bytes: &[u8]) -> Result<Theme, ImportError> {
    let mut reader = BufReader::new(Cursor::new(bytes));
    ThemeSet::load_from_reader(&mut reader).map_err(|error| ImportError::Parse {
        path: path.to_owned(),
        message: error.to_string(),
    })
}

#[derive(Debug, Default, Deserialize)]
struct ColorScheme {
    name: Option<String>,
    author: Option<String>,
    #[serde(default)]
    variables: HashMap<String, String>,
    #[serde(default)]
    globals: HashMap<String, Value>,
    #[serde(default)]
    rules: Vec<ColorRule>,
}

#[derive(Debug, Default, Deserialize)]
struct ColorRule {
    #[serde(default)]
    scope: Value,
    foreground: Option<String>,
    background: Option<String>,
    font_style: Option<String>,
}

fn parse_color_scheme(path: &Path, bytes: &[u8]) -> Result<Theme, ImportError> {
    let source = std::str::from_utf8(bytes).map_err(|error| ImportError::Parse {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    let compatible_json = json_compatible(source).map_err(|message| ImportError::Parse {
        path: path.to_owned(),
        message,
    })?;
    let scheme: ColorScheme =
        serde_json::from_str(&compatible_json).map_err(|error| ImportError::Parse {
            path: path.to_owned(),
            message: error.to_string(),
        })?;

    let color = |key: &str| {
        scheme
            .globals
            .get(key)
            .and_then(Value::as_str)
            .and_then(|value| parse_color(value, &scheme.variables))
            .map(Into::into)
    };
    let settings = syntect::highlighting::ThemeSettings {
        foreground: color("foreground"),
        background: color("background"),
        caret: color("caret"),
        line_highlight: color("line_highlight"),
        misspelling: color("misspelling"),
        accent: color("accent"),
        bracket_contents_foreground: color("bracket_contents_foreground"),
        brackets_foreground: color("brackets_foreground"),
        brackets_background: color("brackets_background"),
        tags_foreground: color("tags_foreground"),
        highlight: color("highlight"),
        find_highlight: color("find_highlight"),
        find_highlight_foreground: color("find_highlight_foreground"),
        gutter: color("gutter"),
        gutter_foreground: color("gutter_foreground"),
        selection: color("selection"),
        selection_foreground: color("selection_foreground"),
        selection_border: color("selection_border"),
        inactive_selection: color("inactive_selection"),
        inactive_selection_foreground: color("inactive_selection_foreground"),
        guide: color("guide"),
        active_guide: color("active_guide"),
        stack_guide: color("stack_guide"),
        shadow: color("shadow"),
        ..Default::default()
    };

    let scopes = scheme
        .rules
        .into_iter()
        .filter_map(|rule| convert_rule(rule, &scheme.variables))
        .collect();
    Ok(Theme {
        name: scheme.name,
        author: scheme.author,
        settings,
        scopes,
    })
}

fn convert_rule(rule: ColorRule, variables: &HashMap<String, String>) -> Option<ThemeItem> {
    let scope = match rule.scope {
        Value::String(scope) => scope,
        Value::Array(scopes) => scopes
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        _ => return None,
    };
    let scope = ScopeSelectors::from_str(&scope).ok()?;
    let style = StyleModifier {
        foreground: rule
            .foreground
            .as_deref()
            .and_then(|value| parse_color(value, variables))
            .map(Into::into),
        background: rule
            .background
            .as_deref()
            .and_then(|value| parse_color(value, variables))
            .map(Into::into),
        font_style: rule.font_style.as_deref().map(parse_font_style),
    };
    (style.foreground.is_some() || style.background.is_some() || style.font_style.is_some())
        .then_some(ThemeItem { scope, style })
}

fn parse_font_style(value: &str) -> FontStyle {
    value
        .split_ascii_whitespace()
        .fold(FontStyle::empty(), |style, token| {
            style
                | match token {
                    "bold" => FontStyle::BOLD,
                    "italic" => FontStyle::ITALIC,
                    "underline" => FontStyle::UNDERLINE,
                    _ => FontStyle::empty(),
                }
        })
}

fn derive_palette(theme: &Theme, name: Option<&str>) -> (bool, SemanticPalette) {
    let named_dark = name.is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        ["dark", "night", "black", "midnight"]
            .iter()
            .any(|word| name.contains(word))
    });
    let raw_background = theme
        .settings
        .background
        .map(Into::into)
        .unwrap_or(if named_dark {
            Rgba::rgb(30, 34, 43)
        } else {
            Rgba::rgb(250, 250, 252)
        });
    let editor_background = Rgba {
        a: 255,
        ..raw_background
    };
    let dark_mode = editor_background.relative_luminance() < 0.35;
    let defaults = Defaults::new(dark_mode);
    let highlighter = Highlighter::new(theme);
    let scoped = |scope: &str, fallback: Rgba| {
        ScopeStack::from_str(scope)
            .ok()
            .filter(|stack| {
                theme.scopes.iter().any(|item| {
                    item.style.foreground.is_some()
                        && item.scope.does_match(stack.as_slice()).is_some()
                })
            })
            .map(|stack| Rgba::from(highlighter.style_for_stack(stack.as_slice()).foreground))
            .unwrap_or(fallback)
    };
    let foreground = readable(
        theme
            .settings
            .foreground
            .map(Into::into)
            .unwrap_or(defaults.foreground),
        editor_background,
    );
    let plain = foreground;
    let keyword = readable(
        scoped("keyword.control", defaults.keyword),
        editor_background,
    );
    let comment = readable(scoped("comment.line", defaults.muted), editor_background);
    let string = readable(scoped("string.quoted", defaults.string), editor_background);
    let accent = readable(
        theme
            .settings
            .accent
            .or(theme.settings.caret)
            .map(Into::into)
            .unwrap_or(keyword),
        editor_background,
    );
    let muted = readable(
        theme
            .settings
            .gutter_foreground
            .map(Into::into)
            .unwrap_or(comment),
        editor_background,
    );
    let error = readable(
        scoped(
            "invalid.illegal",
            theme
                .settings
                .misspelling
                .map(Into::into)
                .unwrap_or(defaults.error),
        ),
        editor_background,
    );
    let warning = readable(defaults.warning, editor_background);
    let info = readable(defaults.info, editor_background);
    let success = readable(defaults.success, editor_background);
    let surface = editor_background.mix(foreground, 0.04);
    let elevated_surface = editor_background.mix(foreground, 0.075);
    let selection = theme
        .settings
        .selection
        .map(Into::into)
        .unwrap_or(accent.with_alpha(72));
    let selection_background = selection.composite_over(editor_background);

    let palette = SemanticPalette {
        background: editor_background,
        surface,
        elevated_surface,
        foreground,
        muted,
        border: editor_background.mix(foreground, 0.18),
        accent,
        error,
        warning,
        info,
        success,
        editor_background,
        current_line: theme
            .settings
            .line_highlight
            .map(Into::into)
            .unwrap_or(accent.with_alpha(if dark_mode { 25 } else { 18 })),
        selection,
        selection_foreground: readable(
            theme
                .settings
                .selection_foreground
                .map(Into::into)
                .unwrap_or(foreground),
            selection_background,
        ),
        caret: readable(
            theme.settings.caret.map(Into::into).unwrap_or(accent),
            editor_background,
        ),
        gutter_background: theme.settings.gutter.map(Into::into).unwrap_or(surface),
        gutter_foreground: muted,
        plain,
        comment,
        operator: readable(
            scoped("keyword.operator", defaults.operator),
            editor_background,
        ),
        number: readable(
            scoped("constant.numeric", defaults.number),
            editor_background,
        ),
        emphasis: readable(
            scoped("markup.italic", defaults.emphasis),
            editor_background,
        ),
        link: readable(
            scoped("markup.underline.link", defaults.link),
            editor_background,
        ),
        string,
        label: readable(
            scoped("entity.name.label", defaults.label),
            editor_background,
        ),
        heading: readable(
            scoped("markup.heading", defaults.heading),
            editor_background,
        ),
        keyword,
        interpolated: readable(
            scoped("meta.interpolation", defaults.interpolated),
            editor_background,
        ),
        error_background: error.with_alpha(if dark_mode { 48 } else { 28 }),
    };
    (dark_mode, palette)
}

struct Defaults {
    foreground: Rgba,
    muted: Rgba,
    operator: Rgba,
    number: Rgba,
    emphasis: Rgba,
    link: Rgba,
    string: Rgba,
    label: Rgba,
    heading: Rgba,
    keyword: Rgba,
    interpolated: Rgba,
    error: Rgba,
    warning: Rgba,
    info: Rgba,
    success: Rgba,
}

impl Defaults {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                foreground: Rgba::rgb(214, 219, 230),
                muted: Rgba::rgb(166, 173, 186),
                operator: Rgba::rgb(145, 215, 227),
                number: Rgba::rgb(245, 169, 127),
                emphasis: Rgba::rgb(244, 184, 228),
                link: Rgba::rgb(125, 196, 228),
                string: Rgba::rgb(166, 218, 149),
                label: Rgba::rgb(139, 213, 202),
                heading: Rgba::rgb(238, 212, 159),
                keyword: Rgba::rgb(198, 160, 246),
                interpolated: Rgba::rgb(183, 189, 248),
                error: Rgba::rgb(237, 135, 150),
                warning: Rgba::rgb(238, 212, 159),
                info: Rgba::rgb(125, 196, 228),
                success: Rgba::rgb(166, 218, 149),
            }
        } else {
            Self {
                foreground: Rgba::rgb(52, 58, 70),
                muted: Rgba::rgb(82, 88, 99),
                operator: Rgba::rgb(26, 112, 146),
                number: Rgba::rgb(190, 88, 40),
                emphasis: Rgba::rgb(158, 53, 137),
                link: Rgba::rgb(26, 112, 146),
                string: Rgba::rgb(58, 128, 78),
                label: Rgba::rgb(20, 122, 111),
                heading: Rgba::rgb(145, 93, 16),
                keyword: Rgba::rgb(126, 69, 174),
                interpolated: Rgba::rgb(89, 77, 150),
                error: Rgba::rgb(176, 36, 55),
                warning: Rgba::rgb(145, 91, 10),
                info: Rgba::rgb(0, 102, 148),
                success: Rgba::rgb(43, 120, 48),
            }
        }
    }
}

fn readable(color: Rgba, background: Rgba) -> Rgba {
    if color.contrast_ratio(background) >= MIN_TEXT_CONTRAST {
        return color;
    }
    let color = color.composite_over(background);
    let black_ratio = Rgba::BLACK.contrast_ratio(background);
    let white_ratio = Rgba::WHITE.contrast_ratio(background);
    let target = if black_ratio > white_ratio {
        Rgba::BLACK
    } else {
        Rgba::WHITE
    };
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..12 {
        let middle = (low + high) / 2.0;
        if color.mix(target, middle).contrast_ratio(background) >= MIN_TEXT_CONTRAST {
            high = middle;
        } else {
            low = middle;
        }
    }
    color.mix(target, high)
}

fn mix_channel(left: u8, right: u8, amount: f32) -> u8 {
    (f32::from(left) + (f32::from(right) - f32::from(left)) * amount).round() as u8
}

fn parse_color(value: &str, variables: &HashMap<String, String>) -> Option<Rgba> {
    parse_color_inner(value.trim(), variables, 0)
}

fn parse_color_inner(
    value: &str,
    variables: &HashMap<String, String>,
    depth: usize,
) -> Option<Rgba> {
    if depth >= MAX_VARIABLE_DEPTH {
        return None;
    }
    let value = value.trim();
    if let Some(name) = wrapped(value, "var") {
        return variables
            .get(name.trim())
            .and_then(|value| parse_color_inner(value, variables, depth + 1));
    }
    if let Some(expression) = wrapped(value, "color") {
        let (base, modifiers) = split_expression(expression)?;
        let mut color = parse_color_inner(base, variables, depth + 1)?;
        for modifier in split_modifiers(modifiers) {
            if let Some(alpha) = wrapped(modifier, "alpha").and_then(parse_unit_interval) {
                color.a = (alpha * 255.0).round() as u8;
            } else if let Some(blend) = wrapped(modifier, "blend") {
                let (other, amount) = split_last_argument(blend)?;
                color = color.mix(
                    parse_color_inner(other, variables, depth + 1)?,
                    parse_unit_interval(amount)?,
                );
            }
        }
        return Some(color);
    }
    if value.starts_with('#') {
        return parse_hex(value);
    }
    for (function, has_alpha) in [("rgba", true), ("rgb", false)] {
        if let Some(arguments) = wrapped(value, function) {
            return parse_rgb(arguments, has_alpha);
        }
    }
    for (function, has_alpha) in [("hsla", true), ("hsl", false)] {
        if let Some(arguments) = wrapped(value, function) {
            return parse_hsl(arguments, has_alpha);
        }
    }
    match value.to_ascii_lowercase().as_str() {
        "black" => Some(Rgba::BLACK),
        "white" => Some(Rgba::WHITE),
        "transparent" => Some(Rgba::from_rgba(0, 0, 0, 0)),
        _ => None,
    }
}

fn wrapped<'a>(value: &'a str, function: &str) -> Option<&'a str> {
    value
        .strip_prefix(function)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

fn parse_hex(value: &str) -> Option<Rgba> {
    fn nibble(value: u8) -> Option<u8> {
        (value as char).to_digit(16).map(|value| value as u8)
    }
    fn pair(value: &[u8]) -> Option<u8> {
        Some(nibble(value[0])? * 16 + nibble(value[1])?)
    }
    let value = value.strip_prefix('#')?.as_bytes();
    match value.len() {
        3 => Some(Rgba::rgb(
            nibble(value[0])? * 17,
            nibble(value[1])? * 17,
            nibble(value[2])? * 17,
        )),
        4 => Some(Rgba::from_rgba(
            nibble(value[0])? * 17,
            nibble(value[1])? * 17,
            nibble(value[2])? * 17,
            nibble(value[3])? * 17,
        )),
        6 => Some(Rgba::rgb(
            pair(&value[0..2])?,
            pair(&value[2..4])?,
            pair(&value[4..6])?,
        )),
        8 => Some(Rgba::from_rgba(
            pair(&value[0..2])?,
            pair(&value[2..4])?,
            pair(&value[4..6])?,
            pair(&value[6..8])?,
        )),
        _ => None,
    }
}

fn color_arguments(value: &str) -> Vec<&str> {
    value
        .split([',', ' ', '/'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_rgb(value: &str, has_alpha: bool) -> Option<Rgba> {
    let values = color_arguments(value);
    if values.len() != 3 + usize::from(has_alpha) {
        return None;
    }
    let channel = |value: &str| {
        if let Some(percent) = value.strip_suffix('%') {
            Some(
                (percent.parse::<f32>().ok()? * 2.55)
                    .round()
                    .clamp(0.0, 255.0) as u8,
            )
        } else {
            Some(value.parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8)
        }
    };
    Some(Rgba::from_rgba(
        channel(values[0])?,
        channel(values[1])?,
        channel(values[2])?,
        if has_alpha {
            (parse_unit_interval(values[3])? * 255.0).round() as u8
        } else {
            255
        },
    ))
}

fn parse_hsl(value: &str, has_alpha: bool) -> Option<Rgba> {
    let values = color_arguments(value);
    if values.len() != 3 + usize::from(has_alpha) {
        return None;
    }
    let hue = values[0]
        .trim_end_matches("deg")
        .parse::<f32>()
        .ok()?
        .rem_euclid(360.0)
        / 360.0;
    let saturation = parse_percentage(values[1])?;
    let lightness = parse_percentage(values[2])?;
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let hue_sector = hue * 6.0;
    let secondary = chroma * (1.0 - (hue_sector.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match hue_sector as u8 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let offset = lightness - chroma / 2.0;
    Some(Rgba::from_rgba(
        ((red + offset) * 255.0).round() as u8,
        ((green + offset) * 255.0).round() as u8,
        ((blue + offset) * 255.0).round() as u8,
        if has_alpha {
            (parse_unit_interval(values[3])? * 255.0).round() as u8
        } else {
            255
        },
    ))
}

fn parse_percentage(value: &str) -> Option<f32> {
    Some(
        value
            .strip_suffix('%')?
            .parse::<f32>()
            .ok()?
            .clamp(0.0, 100.0)
            / 100.0,
    )
}

fn parse_unit_interval(value: &str) -> Option<f32> {
    if let Some(percent) = value.trim().strip_suffix('%') {
        Some(percent.parse::<f32>().ok()?.clamp(0.0, 100.0) / 100.0)
    } else {
        Some(value.trim().parse::<f32>().ok()?.clamp(0.0, 1.0))
    }
}

fn split_expression(value: &str) -> Option<(&str, &str)> {
    let mut depth = 0;
    for (index, character) in value.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth -= 1,
            character if character.is_whitespace() && depth == 0 => {
                return Some((&value[..index], value[index..].trim()));
            }
            _ => {}
        }
    }
    Some((value, ""))
}

fn split_modifiers(mut value: &str) -> Vec<&str> {
    let mut modifiers = Vec::new();
    while !value.is_empty() {
        let mut depth = 0;
        let mut end = value.len();
        for (index, character) in value.char_indices() {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = index + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        modifiers.push(&value[..end]);
        value = value[end..].trim_start();
    }
    modifiers
}

fn split_last_argument(value: &str) -> Option<(&str, &str)> {
    let index = value.rfind(char::is_whitespace)?;
    Some((value[..index].trim(), value[index..].trim()))
}

/// Removes Sublime's JSON comments and trailing commas while preserving text
/// inside strings. Keeping this adapter local avoids a second JSON parser.
fn json_compatible(source: &str) -> Result<String, String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        String,
        LineComment,
        BlockComment,
    }

    let mut state = State::Normal;
    let mut escaped = false;
    let mut without_comments = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    while let Some(character) = characters.next() {
        match state {
            State::Normal if character == '"' => {
                state = State::String;
                without_comments.push(character);
            }
            State::Normal if character == '/' && characters.peek() == Some(&'/') => {
                characters.next();
                state = State::LineComment;
                without_comments.push(' ');
                without_comments.push(' ');
            }
            State::Normal if character == '/' && characters.peek() == Some(&'*') => {
                characters.next();
                state = State::BlockComment;
                without_comments.push(' ');
                without_comments.push(' ');
            }
            State::Normal => without_comments.push(character),
            State::String => {
                without_comments.push(character);
                if character == '"' && !escaped {
                    state = State::Normal;
                }
                escaped = character == '\\' && !escaped;
                if character != '\\' {
                    escaped = false;
                }
            }
            State::LineComment if character == '\n' => {
                state = State::Normal;
                without_comments.push('\n');
            }
            State::LineComment => without_comments.push(' '),
            State::BlockComment if character == '*' && characters.peek() == Some(&'/') => {
                characters.next();
                without_comments.push(' ');
                without_comments.push(' ');
                state = State::Normal;
            }
            State::BlockComment => {
                without_comments.push(if character == '\n' { '\n' } else { ' ' })
            }
        }
    }
    if state == State::BlockComment {
        return Err("unterminated block comment".to_owned());
    }

    let characters: Vec<char> = without_comments.chars().collect();
    let mut compatible = String::with_capacity(without_comments.len());
    let mut in_string = false;
    let mut escaped = false;
    for (index, &character) in characters.iter().enumerate() {
        if character == '"' && !escaped {
            in_string = !in_string;
        }
        if character == ',' && !in_string {
            let next = characters[index + 1..]
                .iter()
                .copied()
                .find(|character| !character.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        compatible.push(character);
        escaped = in_string && character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }
    Ok(compatible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_sublime_color_expressions() {
        let variables = HashMap::from([
            ("blue".to_owned(), "#4080ff".to_owned()),
            ("nested".to_owned(), "var(blue)".to_owned()),
        ]);
        assert_eq!(
            parse_color("#abc", &variables),
            Some(Rgba::rgb(170, 187, 204))
        );
        assert_eq!(
            parse_color("#abcd", &variables),
            Some(Rgba::from_rgba(170, 187, 204, 221))
        );
        assert_eq!(
            parse_color("rgb(10, 20, 30)", &variables),
            Some(Rgba::rgb(10, 20, 30))
        );
        assert_eq!(
            parse_color("hsl(0, 100%, 50%)", &variables),
            Some(Rgba::rgb(255, 0, 0))
        );
        assert_eq!(
            parse_color("color(var(nested) alpha(25%))", &variables),
            Some(Rgba::from_rgba(64, 128, 255, 64))
        );
    }

    #[test]
    fn comment_adapter_does_not_damage_strings() {
        let source = r#"{"url":"https://example.test/*keep*/",/*drop*/"value":1,}"#;
        let value: Value = serde_json::from_str(&json_compatible(source).unwrap()).unwrap();
        assert_eq!(value["url"], "https://example.test/*keep*/");
        assert_eq!(value["value"], 1);
    }

    #[test]
    fn cyclic_variables_fall_back_instead_of_recursing_forever() {
        let variables = HashMap::from([
            ("a".to_owned(), "var(b)".to_owned()),
            ("b".to_owned(), "var(a)".to_owned()),
        ]);
        assert_eq!(parse_color("var(a)", &variables), None);
    }
}
