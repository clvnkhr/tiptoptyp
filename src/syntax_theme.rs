//! Persisted Typst-specific overrides and their resolved editor formats.
//!
//! Themes remain the source of default colors and TextMate decorations. An
//! override only replaces the fields the user explicitly enabled, so changing
//! the selected light or dark theme continues to update every inherited role.

use std::{collections::BTreeMap, str::FromStr};

use eframe::egui::{Color32, Stroke, TextFormat};
use serde::{Deserialize, Serialize};
use syntect::{
    highlighting::{FontStyle, Highlighter, Theme},
    parsing::ScopeStack,
};
use typst_syntax::Tag;

use crate::{
    sublime_theme::Rgba,
    theme::{self, METRICS, SyntaxPalette},
};

/// Stable application-facing names for plain text and every Typst highlight
/// tag. Keeping these names independent of `typst-syntax` makes the persisted
/// format explicit when Typst adds a new tag in a future release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub(crate) enum TypstSyntaxRole {
    Plain,
    Comment,
    Punctuation,
    Escape,
    Strong,
    Emphasis,
    Link,
    Raw,
    Label,
    Reference,
    Heading,
    ListMarker,
    ListTerm,
    MathDelimiter,
    MathOperator,
    MathGroupingParens,
    Keyword,
    Operator,
    Number,
    String,
    Function,
    Interpolated,
    Error,
}

impl TypstSyntaxRole {
    pub(crate) const ALL: [Self; 23] = [
        Self::Plain,
        Self::Comment,
        Self::Punctuation,
        Self::Escape,
        Self::Strong,
        Self::Emphasis,
        Self::Link,
        Self::Raw,
        Self::Label,
        Self::Reference,
        Self::Heading,
        Self::ListMarker,
        Self::ListTerm,
        Self::MathDelimiter,
        Self::MathOperator,
        Self::MathGroupingParens,
        Self::Keyword,
        Self::Operator,
        Self::Number,
        Self::String,
        Self::Function,
        Self::Interpolated,
        Self::Error,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Plain => "Plain text",
            Self::Comment => "Comment",
            Self::Punctuation => "Punctuation",
            Self::Escape => "Escape",
            Self::Strong => "Strong",
            Self::Emphasis => "Emphasis",
            Self::Link => "Link",
            Self::Raw => "Raw text",
            Self::Label => "Label",
            Self::Reference => "Reference",
            Self::Heading => "Heading",
            Self::ListMarker => "List marker",
            Self::ListTerm => "List term",
            Self::MathDelimiter => "Math delimiter",
            Self::MathOperator => "Math operator",
            Self::MathGroupingParens => "Math parentheses",
            Self::Keyword => "Keyword",
            Self::Operator => "Operator",
            Self::Number => "Number",
            Self::String => "String",
            Self::Function => "Function",
            Self::Interpolated => "Interpolated",
            Self::Error => "Error",
        }
    }

    pub(crate) const fn group(self) -> &'static str {
        match self {
            Self::Plain
            | Self::Comment
            | Self::Strong
            | Self::Emphasis
            | Self::Link
            | Self::Raw
            | Self::Label
            | Self::Reference
            | Self::Heading
            | Self::ListMarker
            | Self::ListTerm => "Markup",
            Self::MathDelimiter | Self::MathOperator | Self::MathGroupingParens => "Math",
            Self::Punctuation
            | Self::Escape
            | Self::Keyword
            | Self::Operator
            | Self::Number
            | Self::String
            | Self::Function
            | Self::Interpolated => "Code",
            Self::Error => "Diagnostics",
        }
    }

    pub(crate) const fn sample(self) -> &'static str {
        match self {
            Self::Plain => "Document text",
            Self::Comment => "// a comment",
            Self::Punctuation => "(value: 1)",
            Self::Escape => "\\#",
            Self::Strong => "*strong*",
            Self::Emphasis => "_emphasis_",
            Self::Link => "https://typst.app",
            Self::Raw => "`raw text`",
            Self::Label => "<chapter>",
            Self::Reference => "@chapter",
            Self::Heading => "= Heading",
            Self::ListMarker => "- item",
            Self::ListTerm => "/ term: detail",
            Self::MathDelimiter => "$ x $",
            Self::MathOperator => "x + y",
            Self::MathGroupingParens => "(x)",
            Self::Keyword => "let value",
            Self::Operator => "x => y",
            Self::Number => "42.5",
            Self::String => "\"text\"",
            Self::Function => "text(..)",
            Self::Interpolated => "#value",
            Self::Error => "invalid syntax",
        }
    }

    pub(crate) const fn from_tag(tag: Tag) -> Self {
        match tag {
            Tag::Comment => Self::Comment,
            Tag::Punctuation => Self::Punctuation,
            Tag::Escape => Self::Escape,
            Tag::Strong => Self::Strong,
            Tag::Emph => Self::Emphasis,
            Tag::Link => Self::Link,
            Tag::Raw => Self::Raw,
            Tag::Label => Self::Label,
            Tag::Ref => Self::Reference,
            Tag::Heading => Self::Heading,
            Tag::ListMarker => Self::ListMarker,
            Tag::ListTerm => Self::ListTerm,
            Tag::MathDelimiter => Self::MathDelimiter,
            Tag::MathOperator => Self::MathOperator,
            Tag::MathGroupingParens => Self::MathGroupingParens,
            Tag::Keyword => Self::Keyword,
            Tag::Operator => Self::Operator,
            Tag::Number => Self::Number,
            Tag::String => Self::String,
            Tag::Function => Self::Function,
            Tag::Interpolated => Self::Interpolated,
            Tag::Error => Self::Error,
        }
    }

    const fn tag(self) -> Option<Tag> {
        match self {
            Self::Plain => None,
            Self::Comment => Some(Tag::Comment),
            Self::Punctuation => Some(Tag::Punctuation),
            Self::Escape => Some(Tag::Escape),
            Self::Strong => Some(Tag::Strong),
            Self::Emphasis => Some(Tag::Emph),
            Self::Link => Some(Tag::Link),
            Self::Raw => Some(Tag::Raw),
            Self::Label => Some(Tag::Label),
            Self::Reference => Some(Tag::Ref),
            Self::Heading => Some(Tag::Heading),
            Self::ListMarker => Some(Tag::ListMarker),
            Self::ListTerm => Some(Tag::ListTerm),
            Self::MathDelimiter => Some(Tag::MathDelimiter),
            Self::MathOperator => Some(Tag::MathOperator),
            Self::MathGroupingParens => Some(Tag::MathGroupingParens),
            Self::Keyword => Some(Tag::Keyword),
            Self::Operator => Some(Tag::Operator),
            Self::Number => Some(Tag::Number),
            Self::String => Some(Tag::String),
            Self::Function => Some(Tag::Function),
            Self::Interpolated => Some(Tag::Interpolated),
            Self::Error => Some(Tag::Error),
        }
    }
}

/// One optional layer over a selected theme. `None` means inherit; an explicit
/// `Some(false)` is therefore distinct from inheriting a bold/italic theme
/// rule.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TypstStyleOverride {
    pub(crate) foreground: Option<Rgba>,
    pub(crate) background: Option<Rgba>,
    pub(crate) bold: Option<bool>,
    pub(crate) italic: Option<bool>,
    pub(crate) underline: Option<bool>,
    pub(crate) strikethrough: Option<bool>,
}

impl TypstStyleOverride {
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TypstStyleOverrides {
    styles: BTreeMap<TypstSyntaxRole, TypstStyleOverride>,
}

impl TypstStyleOverrides {
    pub(crate) fn get(&self, role: TypstSyntaxRole) -> Option<&TypstStyleOverride> {
        self.styles.get(&role)
    }

    pub(crate) fn get_mut_or_default(&mut self, role: TypstSyntaxRole) -> &mut TypstStyleOverride {
        self.styles.entry(role).or_default()
    }

    pub(crate) fn set(&mut self, role: TypstSyntaxRole, style: TypstStyleOverride) {
        if style.is_empty() {
            self.styles.remove(&role);
        } else {
            self.styles.insert(role, style);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.styles.clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }
}

/// Independent overrides keep absolute colors readable in both appearances.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TypstOverrideThemes {
    pub(crate) light: TypstStyleOverrides,
    pub(crate) dark: TypstStyleOverrides,
}

impl TypstOverrideThemes {
    pub(crate) fn for_dark(&self, dark: bool) -> &TypstStyleOverrides {
        if dark { &self.dark } else { &self.light }
    }

    pub(crate) fn for_dark_mut(&mut self, dark: bool) -> &mut TypstStyleOverrides {
        if dark {
            &mut self.dark
        } else {
            &mut self.light
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedTypstStyle {
    pub(crate) foreground: Color32,
    pub(crate) background: Color32,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) strikethrough: bool,
}

impl ResolvedTypstStyle {
    pub(crate) fn text_format(self) -> TextFormat {
        TextFormat {
            font_id: theme::editor_font_with_weight(self.bold),
            color: self.foreground,
            background: self.background,
            italics: self.italic,
            underline: if self.underline {
                Stroke::new(METRICS.syntax.link_underline_width, self.foreground)
            } else {
                Stroke::NONE
            },
            strikethrough: if self.strikethrough {
                Stroke::new(METRICS.syntax.link_underline_width, self.foreground)
            } else {
                Stroke::NONE
            },
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedTypstStyles {
    styles: [ResolvedTypstStyle; TypstSyntaxRole::ALL.len()],
}

impl ResolvedTypstStyles {
    pub(crate) fn resolve(
        palette: SyntaxPalette,
        syntect_theme: Option<&Theme>,
        overrides: &TypstStyleOverrides,
    ) -> Self {
        let styles = std::array::from_fn(|index| {
            let role = TypstSyntaxRole::ALL[index];
            resolve_role(role, palette, syntect_theme, overrides.get(role))
        });
        Self { styles }
    }

    pub(crate) fn style(&self, role: TypstSyntaxRole) -> ResolvedTypstStyle {
        self.styles[role as usize]
    }

    pub(crate) fn resolve_style(
        role: TypstSyntaxRole,
        palette: SyntaxPalette,
        syntect_theme: Option<&Theme>,
        overrides: Option<&TypstStyleOverride>,
    ) -> ResolvedTypstStyle {
        resolve_role(role, palette, syntect_theme, overrides)
    }

    pub(crate) fn format(&self, tag: Option<Tag>) -> TextFormat {
        self.style(tag.map_or(TypstSyntaxRole::Plain, TypstSyntaxRole::from_tag))
            .text_format()
    }
}

impl Default for ResolvedTypstStyles {
    fn default() -> Self {
        Self::resolve(theme::syntax_palette(true), None, &Default::default())
    }
}

fn resolve_role(
    role: TypstSyntaxRole,
    palette: SyntaxPalette,
    syntect_theme: Option<&Theme>,
    overrides: Option<&TypstStyleOverride>,
) -> ResolvedTypstStyle {
    let mut style = ResolvedTypstStyle {
        foreground: role_color(role, palette),
        background: if role == TypstSyntaxRole::Error {
            palette.error_background
        } else {
            Color32::TRANSPARENT
        },
        bold: role == TypstSyntaxRole::Strong,
        italic: role == TypstSyntaxRole::Emphasis,
        underline: role == TypstSyntaxRole::Link,
        strikethrough: false,
    };

    if let (Some(theme), Some(tag)) = (syntect_theme, role.tag())
        && let Ok(stack) = ScopeStack::from_str(tag.tm_scope())
    {
        let stack = stack.as_slice();
        let matched_foreground = theme
            .scopes
            .iter()
            .any(|item| item.style.foreground.is_some() && item.scope.does_match(stack).is_some());
        let matched_background = theme
            .scopes
            .iter()
            .any(|item| item.style.background.is_some() && item.scope.does_match(stack).is_some());
        let matched_font = theme
            .scopes
            .iter()
            .any(|item| item.style.font_style.is_some() && item.scope.does_match(stack).is_some());
        let themed = Highlighter::new(theme).style_for_stack(stack);
        if matched_foreground {
            style.foreground = syntect_color(themed.foreground);
        }
        if matched_background {
            style.background = syntect_color(themed.background);
        }
        if matched_font {
            style.bold = themed.font_style.contains(FontStyle::BOLD);
            style.italic = themed.font_style.contains(FontStyle::ITALIC);
            style.underline = themed.font_style.contains(FontStyle::UNDERLINE);
        }
    }

    if let Some(overrides) = overrides {
        if let Some(color) = overrides.foreground {
            style.foreground = rgba(color);
        }
        if let Some(color) = overrides.background {
            style.background = rgba(color);
        }
        if let Some(value) = overrides.bold {
            style.bold = value;
        }
        if let Some(value) = overrides.italic {
            style.italic = value;
        }
        if let Some(value) = overrides.underline {
            style.underline = value;
        }
        if let Some(value) = overrides.strikethrough {
            style.strikethrough = value;
        }
    }
    style
}

fn role_color(role: TypstSyntaxRole, colors: SyntaxPalette) -> Color32 {
    match role {
        TypstSyntaxRole::Plain => colors.plain,
        TypstSyntaxRole::Comment => colors.comment,
        TypstSyntaxRole::Punctuation
        | TypstSyntaxRole::MathGroupingParens
        | TypstSyntaxRole::Operator => colors.operator,
        TypstSyntaxRole::Escape | TypstSyntaxRole::Number => colors.number,
        TypstSyntaxRole::Strong
        | TypstSyntaxRole::Emphasis
        | TypstSyntaxRole::MathDelimiter
        | TypstSyntaxRole::MathOperator => colors.emphasis,
        TypstSyntaxRole::Link | TypstSyntaxRole::Function => colors.link,
        TypstSyntaxRole::Raw | TypstSyntaxRole::String => colors.string,
        TypstSyntaxRole::Label | TypstSyntaxRole::Reference => colors.label,
        TypstSyntaxRole::Heading | TypstSyntaxRole::ListMarker | TypstSyntaxRole::ListTerm => {
            colors.heading
        }
        TypstSyntaxRole::Keyword => colors.keyword,
        TypstSyntaxRole::Interpolated => colors.interpolated,
        TypstSyntaxRole::Error => colors.error,
    }
}

fn rgba(color: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn syntect_color(color: syntect::highlighting::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntect::highlighting::{ScopeSelectors, StyleModifier, ThemeItem};

    #[test]
    fn role_catalogue_is_exhaustive_for_typst_tags() {
        assert_eq!(TypstSyntaxRole::ALL.len(), Tag::LIST.len() + 1);
        for tag in Tag::LIST {
            assert!(TypstSyntaxRole::ALL.contains(&TypstSyntaxRole::from_tag(*tag)));
        }
    }

    #[test]
    fn empty_overrides_keep_sensible_markup_defaults() {
        let styles = ResolvedTypstStyles::default();
        assert!(styles.style(TypstSyntaxRole::Strong).bold);
        assert!(styles.style(TypstSyntaxRole::Emphasis).italic);
        assert!(styles.style(TypstSyntaxRole::Link).underline);
        assert_ne!(
            styles.style(TypstSyntaxRole::Error).background,
            Color32::TRANSPARENT
        );
    }

    #[test]
    fn explicit_fields_replace_only_their_inherited_values() {
        let mut overrides = TypstStyleOverrides::default();
        overrides.set(
            TypstSyntaxRole::Strong,
            TypstStyleOverride {
                foreground: Some(Rgba::rgb(1, 2, 3)),
                background: Some(Rgba::from_rgba(4, 5, 6, 70)),
                bold: Some(false),
                italic: Some(true),
                underline: None,
                strikethrough: Some(true),
            },
        );
        let styles = ResolvedTypstStyles::resolve(theme::syntax_palette(true), None, &overrides);
        let style = styles.style(TypstSyntaxRole::Strong);
        assert_eq!(style.foreground, Color32::from_rgb(1, 2, 3));
        assert_eq!(
            style.background,
            Color32::from_rgba_unmultiplied(4, 5, 6, 70)
        );
        assert!(!style.bold);
        assert!(style.italic);
        assert!(!style.underline);
        assert!(style.strikethrough);
    }

    #[test]
    fn light_and_dark_override_maps_are_independent() {
        let mut themes = TypstOverrideThemes::default();
        themes
            .for_dark_mut(true)
            .get_mut_or_default(TypstSyntaxRole::Keyword)
            .bold = Some(true);
        assert!(themes.for_dark(false).is_empty());
        assert!(!themes.for_dark(true).is_empty());
    }

    #[test]
    fn explicit_theme_font_style_replaces_semantic_decoration_defaults() {
        let mut imported = Theme::default();
        imported.scopes.push(ThemeItem {
            scope: ScopeSelectors::from_str(Tag::Strong.tm_scope()).unwrap(),
            style: StyleModifier {
                font_style: Some(FontStyle::ITALIC),
                ..Default::default()
            },
        });
        let styles = ResolvedTypstStyles::resolve(
            theme::syntax_palette(true),
            Some(&imported),
            &Default::default(),
        );
        let strong = styles.style(TypstSyntaxRole::Strong);
        assert!(!strong.bold);
        assert!(strong.italic);
    }
}
