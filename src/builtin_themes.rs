//! Built-in theme catalogue, independent of egui and application state.
//!
//! Each theme resolves to the same semantic roles used by imported Sublime
//! themes. This keeps chrome, editor syntax, diagnostics, and future color
//! transforms on one cohesive boundary.

use std::str::FromStr;

use syntect::highlighting::{
    FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};

use crate::sublime_theme::{Rgba, SemanticPalette};

/// Stable metadata and semantic colors for a bundled theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinTheme {
    /// Stable persistence and command-line identifier.
    pub id: &'static str,
    pub name: &'static str,
    /// Groups light/dark counterparts in pickers.
    pub family: &'static str,
    pub dark_mode: bool,
    pub palette: SemanticPalette,
}

impl BuiltinTheme {
    /// Build a Syntect theme using the same semantic roles as the application.
    pub fn syntect_theme(self) -> Theme {
        syntect_theme(self)
    }
}

#[derive(Clone, Copy)]
struct Recipe {
    background: Rgba,
    surface: Rgba,
    elevated_surface: Rgba,
    foreground: Rgba,
    muted: Rgba,
    border: Rgba,
    accent: Rgba,
    error: Rgba,
    warning: Rgba,
    info: Rgba,
    success: Rgba,
    caret: Rgba,
    current_line: Rgba,
    selection: Rgba,
    comment: Rgba,
    operator: Rgba,
    number: Rgba,
    emphasis: Rgba,
    link: Rgba,
    string: Rgba,
    label: Rgba,
    heading: Rgba,
    keyword: Rgba,
    interpolated: Rgba,
}

const fn rgb(value: u32) -> Rgba {
    Rgba::rgb(
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    )
}

const fn alpha(color: Rgba, value: u8) -> Rgba {
    Rgba::from_rgba(color.r, color.g, color.b, value)
}

const fn semantic(recipe: Recipe, dark_mode: bool) -> SemanticPalette {
    SemanticPalette {
        background: recipe.background,
        surface: recipe.surface,
        elevated_surface: recipe.elevated_surface,
        foreground: recipe.foreground,
        muted: recipe.muted,
        border: recipe.border,
        accent: recipe.accent,
        error: recipe.error,
        warning: recipe.warning,
        info: recipe.info,
        success: recipe.success,
        editor_background: recipe.background,
        current_line: alpha(recipe.current_line, if dark_mode { 26 } else { 20 }),
        selection: alpha(recipe.selection, if dark_mode { 72 } else { 58 }),
        selection_foreground: recipe.foreground,
        caret: recipe.caret,
        gutter_background: recipe.surface,
        gutter_foreground: recipe.muted,
        plain: recipe.foreground,
        comment: recipe.comment,
        operator: recipe.operator,
        number: recipe.number,
        emphasis: recipe.emphasis,
        link: recipe.link,
        string: recipe.string,
        label: recipe.label,
        heading: recipe.heading,
        keyword: recipe.keyword,
        interpolated: recipe.interpolated,
        error_background: alpha(recipe.error, if dark_mode { 34 } else { 24 }),
    }
}

const fn theme(
    id: &'static str,
    name: &'static str,
    family: &'static str,
    dark_mode: bool,
    recipe: Recipe,
) -> BuiltinTheme {
    BuiltinTheme {
        id,
        name,
        family,
        dark_mode,
        palette: semantic(recipe, dark_mode),
    }
}

const TIPTOP_LIGHT: Recipe = Recipe {
    background: rgb(0xfafafc),
    surface: rgb(0xf2f4f7),
    elevated_surface: rgb(0xffffff),
    foreground: rgb(0x343a46),
    muted: rgb(0x787e8c),
    border: rgb(0xd2d6dd),
    accent: rgb(0x4f8cff),
    error: rgb(0xb02437),
    warning: rgb(0x915b0a),
    info: rgb(0x006694),
    success: rgb(0x2b7830),
    caret: rgb(0x276fd1),
    current_line: rgb(0x377ab5),
    selection: rgb(0x4f8cff),
    comment: rgb(0x787e8c),
    operator: rgb(0x1a7092),
    number: rgb(0xbe5828),
    emphasis: rgb(0x9e3589),
    link: rgb(0x1a7092),
    string: rgb(0x3a804e),
    label: rgb(0x147a6f),
    heading: rgb(0x915d10),
    keyword: rgb(0x7e45ae),
    interpolated: rgb(0x594d96),
};

const TIPTOP_DARK: Recipe = Recipe {
    background: rgb(0x1e222b),
    surface: rgb(0x252a35),
    elevated_surface: rgb(0x2d3340),
    foreground: rgb(0xd6dbe6),
    muted: rgb(0xa6adba),
    border: rgb(0x454d5e),
    accent: rgb(0x4f8cff),
    error: rgb(0xed8796),
    warning: rgb(0xeed49f),
    info: rgb(0x7dc4e4),
    success: rgb(0xa6da95),
    caret: rgb(0xb7bdf8),
    current_line: rgb(0x5b8fbe),
    selection: rgb(0x4f8cff),
    comment: rgb(0x6a7a90),
    operator: rgb(0x91d7e3),
    number: rgb(0xf5a97f),
    emphasis: rgb(0xf4b8e4),
    link: rgb(0x7dc4e4),
    string: rgb(0xa6da95),
    label: rgb(0x8bd5ca),
    heading: rgb(0xeed49f),
    keyword: rgb(0xc6a0f6),
    interpolated: rgb(0xb7bdf8),
};

const PAPER_LIGHT: Recipe = Recipe {
    background: rgb(0xf7f3ea),
    surface: rgb(0xeee8dc),
    elevated_surface: rgb(0xfdfaf4),
    foreground: rgb(0x39352f),
    muted: rgb(0x756e64),
    border: rgb(0xcac0b0),
    accent: rgb(0x3867c8),
    error: rgb(0xb12b39),
    warning: rgb(0x865b00),
    info: rgb(0x176b8a),
    success: rgb(0x347443),
    caret: rgb(0x8a4b24),
    current_line: rgb(0xa9733f),
    selection: rgb(0x3867c8),
    comment: rgb(0x7a7165),
    operator: rgb(0x176b8a),
    number: rgb(0xa44f25),
    emphasis: rgb(0x914b77),
    link: rgb(0x285bb5),
    string: rgb(0x397548),
    label: rgb(0x28766d),
    heading: rgb(0x865b00),
    keyword: rgb(0x704c9e),
    interpolated: rgb(0x7b526e),
};

const PAPER_DARK: Recipe = Recipe {
    background: rgb(0x28251f),
    surface: rgb(0x312d26),
    elevated_surface: rgb(0x3b362e),
    foreground: rgb(0xede4d3),
    muted: rgb(0xb1a591),
    border: rgb(0x5b5245),
    accent: rgb(0xd59b5f),
    error: rgb(0xef8c8c),
    warning: rgb(0xe1bd77),
    info: rgb(0x81b7c5),
    success: rgb(0x91bd82),
    caret: rgb(0xf0c9a5),
    current_line: rgb(0xd59b5f),
    selection: rgb(0xd59b5f),
    comment: rgb(0x998e7c),
    operator: rgb(0x81b7c5),
    number: rgb(0xe6a173),
    emphasis: rgb(0xd89cbd),
    link: rgb(0x8fb9e8),
    string: rgb(0x91bd82),
    label: rgb(0x83c1af),
    heading: rgb(0xe1bd77),
    keyword: rgb(0xc5a0df),
    interpolated: rgb(0xd4a7a0),
};

const OCEAN_LIGHT: Recipe = Recipe {
    background: rgb(0xf3f8fa),
    surface: rgb(0xe7f0f3),
    elevated_surface: rgb(0xfbfdfe),
    foreground: rgb(0x253b47),
    muted: rgb(0x607985),
    border: rgb(0xb7cad1),
    accent: rgb(0x147d9a),
    error: rgb(0xb4374e),
    warning: rgb(0x806000),
    info: rgb(0x147d9a),
    success: rgb(0x287c62),
    caret: rgb(0x087f8c),
    current_line: rgb(0x147d9a),
    selection: rgb(0x147d9a),
    comment: rgb(0x6b7f87),
    operator: rgb(0x087f8c),
    number: rgb(0xb25725),
    emphasis: rgb(0x9a447b),
    link: rgb(0x176f9f),
    string: rgb(0x397a56),
    label: rgb(0x1d776e),
    heading: rgb(0x806000),
    keyword: rgb(0x744aa0),
    interpolated: rgb(0x5d5e9f),
};

const OCEAN_DARK: Recipe = Recipe {
    background: rgb(0x102a35),
    surface: rgb(0x153642),
    elevated_surface: rgb(0x1c4350),
    foreground: rgb(0xd8eef2),
    muted: rgb(0x8faeb5),
    border: rgb(0x35606c),
    accent: rgb(0x42b9d1),
    error: rgb(0xf07d8d),
    warning: rgb(0xe7c66c),
    info: rgb(0x73c9d8),
    success: rgb(0x83d1a6),
    caret: rgb(0x8ee2eb),
    current_line: rgb(0x42b9d1),
    selection: rgb(0x42b9d1),
    comment: rgb(0x7f9da5),
    operator: rgb(0x73c9d8),
    number: rgb(0xf1a978),
    emphasis: rgb(0xe4a6cf),
    link: rgb(0x78bff0),
    string: rgb(0x83d1a6),
    label: rgb(0x79d1c2),
    heading: rgb(0xe7c66c),
    keyword: rgb(0xc6a6ea),
    interpolated: rgb(0xb1b8f2),
};

const FOREST_LIGHT: Recipe = Recipe {
    background: rgb(0xf4f7f1),
    surface: rgb(0xe8efe4),
    elevated_surface: rgb(0xfbfcf9),
    foreground: rgb(0x2f3d2e),
    muted: rgb(0x697967),
    border: rgb(0xbccaba),
    accent: rgb(0x3e7c4c),
    error: rgb(0xb53c4c),
    warning: rgb(0x816000),
    info: rgb(0x26768a),
    success: rgb(0x357a3e),
    caret: rgb(0x396f42),
    current_line: rgb(0x3e7c4c),
    selection: rgb(0x3e7c4c),
    comment: rgb(0x71806e),
    operator: rgb(0x26768a),
    number: rgb(0xa6542d),
    emphasis: rgb(0x965078),
    link: rgb(0x276ca0),
    string: rgb(0x357a3e),
    label: rgb(0x24766a),
    heading: rgb(0x816000),
    keyword: rgb(0x70499a),
    interpolated: rgb(0x6f567e),
};

const FOREST_DARK: Recipe = Recipe {
    background: rgb(0x17221a),
    surface: rgb(0x1e2c22),
    elevated_surface: rgb(0x28382c),
    foreground: rgb(0xdce8dc),
    muted: rgb(0x94a894),
    border: rgb(0x405643),
    accent: rgb(0x72b37e),
    error: rgb(0xe27d84),
    warning: rgb(0xd8bd72),
    info: rgb(0x73b7bd),
    success: rgb(0x8bcf8b),
    caret: rgb(0xa5d6a7),
    current_line: rgb(0x72b37e),
    selection: rgb(0x72b37e),
    comment: rgb(0x829482),
    operator: rgb(0x73b7bd),
    number: rgb(0xe7a276),
    emphasis: rgb(0xd6a0bd),
    link: rgb(0x82b4e3),
    string: rgb(0x8bcf8b),
    label: rgb(0x7ac6ac),
    heading: rgb(0xd8bd72),
    keyword: rgb(0xb9a0dc),
    interpolated: rgb(0xc0a6c8),
};

/// Catppuccin values are copied from the official palette v1.8.0 and mapped
/// according to the official editor style guide:
/// <https://github.com/catppuccin/palette/blob/main/palette.json>
/// <https://github.com/catppuccin/catppuccin/blob/main/docs/style-guide.md>
#[derive(Clone, Copy)]
struct CatppuccinColors {
    base: u32,
    mantle: u32,
    surface0: u32,
    overlay0: u32,
    overlay1: u32,
    overlay2: u32,
    text: u32,
    rosewater: u32,
    pink: u32,
    mauve: u32,
    red: u32,
    maroon: u32,
    peach: u32,
    yellow: u32,
    green: u32,
    teal: u32,
    sky: u32,
    blue: u32,
}

const fn catppuccin(colors: CatppuccinColors) -> Recipe {
    Recipe {
        background: rgb(colors.base),
        surface: rgb(colors.mantle),
        elevated_surface: rgb(colors.surface0),
        foreground: rgb(colors.text),
        muted: rgb(colors.overlay1),
        border: rgb(colors.overlay0),
        accent: rgb(colors.blue),
        error: rgb(colors.red),
        warning: rgb(colors.yellow),
        info: rgb(colors.teal),
        success: rgb(colors.green),
        caret: rgb(colors.rosewater),
        current_line: rgb(colors.text),
        selection: rgb(colors.overlay2),
        comment: rgb(colors.overlay2),
        operator: rgb(colors.sky),
        number: rgb(colors.peach),
        emphasis: rgb(colors.pink),
        link: rgb(colors.blue),
        string: rgb(colors.green),
        label: rgb(colors.blue),
        heading: rgb(colors.yellow),
        keyword: rgb(colors.mauve),
        interpolated: rgb(colors.maroon),
    }
}

// Popular paired schemes below are taken from their projects' canonical palettes:
// Solarized: https://github.com/altercation/solarized#the-values
// Gruvbox: https://github.com/morhetz/gruvbox/blob/master/colors/gruvbox.vim
// GitHub: https://github.com/primer/github-vscode-theme/blob/main/src/theme.js
// Rose Pine: https://github.com/rose-pine/neovim/blob/main/lua/rose-pine/palette.lua
// Tokyo Night: https://github.com/tokyo-night/tokyo-night-vscode-theme#color-palette
// Kanagawa: https://github.com/rebelot/kanagawa.nvim/blob/master/lua/kanagawa/colors.lua
// Everforest: https://github.com/sainnhe/everforest/blob/master/autoload/everforest.vim
// Ayu: https://github.com/ayu-theme/ayu-colors/tree/master/themes
// Flexoki: https://github.com/kepano/flexoki/blob/main/README.md
// Dracula/Alucard: https://github.com/dracula/dracula-theme#color-palette-oss
//
// Neutral ramps map to chrome roles in increasing visual prominence. Named hues
// then map consistently across families: red/error, yellow/warning+heading,
// green/success+string, cyan/info+operator, blue/link, purple/keyword, and
// magenta/emphasis+interpolation. The accent is the carrier for selection and
// current-line tints because `semantic` applies the appropriate mode-aware alpha.
#[derive(Clone, Copy)]
struct NamedScheme {
    background: u32,
    surface: u32,
    elevated_surface: u32,
    foreground: u32,
    muted: u32,
    border: u32,
    accent: u32,
    red: u32,
    yellow: u32,
    green: u32,
    blue: u32,
    purple: u32,
    cyan: u32,
    orange: u32,
    magenta: u32,
}

const fn named_scheme(colors: NamedScheme) -> Recipe {
    Recipe {
        background: rgb(colors.background),
        surface: rgb(colors.surface),
        elevated_surface: rgb(colors.elevated_surface),
        foreground: rgb(colors.foreground),
        muted: rgb(colors.muted),
        border: rgb(colors.border),
        accent: rgb(colors.accent),
        error: rgb(colors.red),
        warning: rgb(colors.yellow),
        info: rgb(colors.cyan),
        success: rgb(colors.green),
        caret: rgb(colors.accent),
        current_line: rgb(colors.accent),
        selection: rgb(colors.accent),
        comment: rgb(colors.muted),
        operator: rgb(colors.cyan),
        number: rgb(colors.orange),
        emphasis: rgb(colors.magenta),
        link: rgb(colors.blue),
        string: rgb(colors.green),
        label: rgb(colors.cyan),
        heading: rgb(colors.yellow),
        keyword: rgb(colors.purple),
        interpolated: rgb(colors.magenta),
    }
}

pub const THEMES: &[BuiltinTheme] = &[
    theme(
        "tiptop-light",
        "Tiptop Light",
        "Tiptop",
        false,
        TIPTOP_LIGHT,
    ),
    theme("tiptop-dark", "Tiptop Dark", "Tiptop", true, TIPTOP_DARK),
    theme("paper-light", "Paper Light", "Paper", false, PAPER_LIGHT),
    theme("paper-dark", "Paper Dark", "Paper", true, PAPER_DARK),
    theme("ocean-light", "Ocean Light", "Ocean", false, OCEAN_LIGHT),
    theme("ocean-dark", "Ocean Dark", "Ocean", true, OCEAN_DARK),
    theme(
        "forest-light",
        "Forest Light",
        "Forest",
        false,
        FOREST_LIGHT,
    ),
    theme("forest-dark", "Forest Dark", "Forest", true, FOREST_DARK),
    theme(
        "catppuccin-latte",
        "Catppuccin Latte",
        "Catppuccin",
        false,
        catppuccin(CatppuccinColors {
            base: 0xeff1f5,
            mantle: 0xe6e9ef,
            surface0: 0xccd0da,
            overlay0: 0x9ca0b0,
            overlay1: 0x8c8fa1,
            overlay2: 0x7c7f93,
            text: 0x4c4f69,
            rosewater: 0xdc8a78,
            pink: 0xea76cb,
            mauve: 0x8839ef,
            red: 0xd20f39,
            maroon: 0xe64553,
            peach: 0xfe640b,
            yellow: 0xdf8e1d,
            green: 0x40a02b,
            teal: 0x179299,
            sky: 0x04a5e5,
            blue: 0x1e66f5,
        }),
    ),
    theme(
        "catppuccin-frappe",
        "Catppuccin Frappé",
        "Catppuccin",
        true,
        catppuccin(CatppuccinColors {
            base: 0x303446,
            mantle: 0x292c3c,
            surface0: 0x414559,
            overlay0: 0x737994,
            overlay1: 0x838ba7,
            overlay2: 0x949cbb,
            text: 0xc6d0f5,
            rosewater: 0xf2d5cf,
            pink: 0xf4b8e4,
            mauve: 0xca9ee6,
            red: 0xe78284,
            maroon: 0xea999c,
            peach: 0xef9f76,
            yellow: 0xe5c890,
            green: 0xa6d189,
            teal: 0x81c8be,
            sky: 0x99d1db,
            blue: 0x8caaee,
        }),
    ),
    theme(
        "catppuccin-macchiato",
        "Catppuccin Macchiato",
        "Catppuccin",
        true,
        catppuccin(CatppuccinColors {
            base: 0x24273a,
            mantle: 0x1e2030,
            surface0: 0x363a4f,
            overlay0: 0x6e738d,
            overlay1: 0x8087a2,
            overlay2: 0x939ab7,
            text: 0xcad3f5,
            rosewater: 0xf4dbd6,
            pink: 0xf5bde6,
            mauve: 0xc6a0f6,
            red: 0xed8796,
            maroon: 0xee99a0,
            peach: 0xf5a97f,
            yellow: 0xeed49f,
            green: 0xa6da95,
            teal: 0x8bd5ca,
            sky: 0x91d7e3,
            blue: 0x8aadf4,
        }),
    ),
    theme(
        "catppuccin-mocha",
        "Catppuccin Mocha",
        "Catppuccin",
        true,
        catppuccin(CatppuccinColors {
            base: 0x1e1e2e,
            mantle: 0x181825,
            surface0: 0x313244,
            overlay0: 0x6c7086,
            overlay1: 0x7f849c,
            overlay2: 0x9399b2,
            text: 0xcdd6f4,
            rosewater: 0xf5e0dc,
            pink: 0xf5c2e7,
            mauve: 0xcba6f7,
            red: 0xf38ba8,
            maroon: 0xeba0ac,
            peach: 0xfab387,
            yellow: 0xf9e2af,
            green: 0xa6e3a1,
            teal: 0x94e2d5,
            sky: 0x89dceb,
            blue: 0x89b4fa,
        }),
    ),
    theme(
        "solarized-light",
        "Solarized Light",
        "Solarized",
        false,
        named_scheme(NamedScheme {
            background: 0xfdf6e3,
            surface: 0xeee8d5,
            elevated_surface: 0xfdf6e3,
            // Base01 is the canonical darker text step and meets WCAG AA.
            foreground: 0x586e75,
            muted: 0x657b83,
            border: 0x93a1a1,
            accent: 0x268bd2,
            red: 0xdc322f,
            yellow: 0xb58900,
            green: 0x859900,
            blue: 0x268bd2,
            purple: 0x6c71c4,
            cyan: 0x2aa198,
            orange: 0xcb4b16,
            magenta: 0xd33682,
        }),
    ),
    theme(
        "solarized-dark",
        "Solarized Dark",
        "Solarized",
        true,
        named_scheme(NamedScheme {
            background: 0x002b36,
            surface: 0x073642,
            elevated_surface: 0x073642,
            foreground: 0x839496,
            muted: 0x586e75,
            border: 0x586e75,
            accent: 0x268bd2,
            red: 0xdc322f,
            yellow: 0xb58900,
            green: 0x859900,
            blue: 0x268bd2,
            purple: 0x6c71c4,
            cyan: 0x2aa198,
            orange: 0xcb4b16,
            magenta: 0xd33682,
        }),
    ),
    theme(
        "gruvbox-light",
        "Gruvbox Light",
        "Gruvbox",
        false,
        named_scheme(NamedScheme {
            background: 0xfbf1c7,
            surface: 0xf2e5bc,
            elevated_surface: 0xf9f5d7,
            foreground: 0x3c3836,
            muted: 0x7c6f64,
            border: 0xd5c4a1,
            accent: 0xaf3a03,
            red: 0x9d0006,
            yellow: 0xb57614,
            green: 0x79740e,
            blue: 0x076678,
            purple: 0x8f3f71,
            cyan: 0x427b58,
            orange: 0xaf3a03,
            magenta: 0x8f3f71,
        }),
    ),
    theme(
        "gruvbox-dark",
        "Gruvbox Dark",
        "Gruvbox",
        true,
        named_scheme(NamedScheme {
            background: 0x282828,
            surface: 0x32302f,
            elevated_surface: 0x3c3836,
            foreground: 0xebdbb2,
            muted: 0xa89984,
            border: 0x504945,
            accent: 0xfe8019,
            red: 0xfb4934,
            yellow: 0xfabd2f,
            green: 0xb8bb26,
            blue: 0x83a598,
            purple: 0xd3869b,
            cyan: 0x8ec07c,
            orange: 0xfe8019,
            magenta: 0xd3869b,
        }),
    ),
    theme(
        "github-light-default",
        "GitHub Light Default",
        "GitHub",
        false,
        named_scheme(NamedScheme {
            background: 0xffffff,
            surface: 0xf6f8fa,
            elevated_surface: 0xffffff,
            foreground: 0x1f2328,
            muted: 0x656d76,
            border: 0xd0d7de,
            accent: 0x0969da,
            red: 0xcf222e,
            yellow: 0x9a6700,
            green: 0x1a7f37,
            blue: 0x0969da,
            purple: 0x8250df,
            cyan: 0x1b7c83,
            orange: 0xbc4c00,
            magenta: 0xbf3989,
        }),
    ),
    theme(
        "github-dark-default",
        "GitHub Dark Default",
        "GitHub",
        true,
        named_scheme(NamedScheme {
            background: 0x0d1117,
            surface: 0x161b22,
            elevated_surface: 0x21262d,
            foreground: 0xe6edf3,
            muted: 0x7d8590,
            border: 0x30363d,
            accent: 0x2f81f7,
            red: 0xf85149,
            yellow: 0xd29922,
            green: 0x3fb950,
            blue: 0x58a6ff,
            purple: 0xbc8cff,
            cyan: 0x39c5cf,
            orange: 0xffa657,
            magenta: 0xdb61a2,
        }),
    ),
    theme(
        "rose-pine-dawn",
        "Rosé Pine Dawn",
        "Rosé Pine",
        false,
        named_scheme(NamedScheme {
            background: 0xfaf4ed,
            surface: 0xf2e9e1,
            elevated_surface: 0xfffaf3,
            foreground: 0x464261,
            muted: 0x797593,
            border: 0xcecacd,
            accent: 0x907aa9,
            red: 0xb4637a,
            yellow: 0xea9d34,
            green: 0x6d8f89,
            blue: 0x286983,
            purple: 0x907aa9,
            cyan: 0x56949f,
            orange: 0xd7827e,
            magenta: 0xb4637a,
        }),
    ),
    theme(
        "rose-pine",
        "Rosé Pine",
        "Rosé Pine",
        true,
        named_scheme(NamedScheme {
            background: 0x191724,
            surface: 0x1f1d2e,
            elevated_surface: 0x26233a,
            foreground: 0xe0def4,
            muted: 0x908caa,
            border: 0x403d52,
            accent: 0xc4a7e7,
            red: 0xeb6f92,
            yellow: 0xf6c177,
            green: 0x95b1ac,
            blue: 0x31748f,
            purple: 0xc4a7e7,
            cyan: 0x9ccfd8,
            orange: 0xebbcba,
            magenta: 0xeb6f92,
        }),
    ),
    theme(
        "tokyo-night-light",
        "Tokyo Night Light",
        "Tokyo Night",
        false,
        named_scheme(NamedScheme {
            background: 0xe6e7ed,
            surface: 0xd6d8df,
            elevated_surface: 0xdcdee3,
            foreground: 0x343b58,
            muted: 0x6c6e75,
            border: 0xc1c2c7,
            accent: 0x2959aa,
            red: 0x8c4351,
            yellow: 0x8f5e15,
            green: 0x385f0d,
            blue: 0x2959aa,
            purple: 0x5a3e8e,
            cyan: 0x0f4b6e,
            orange: 0x965027,
            magenta: 0x8c4351,
        }),
    ),
    theme(
        "tokyo-night",
        "Tokyo Night",
        "Tokyo Night",
        true,
        named_scheme(NamedScheme {
            background: 0x1a1b26,
            surface: 0x16161e,
            elevated_surface: 0x20222c,
            foreground: 0xa9b1d6,
            muted: 0x565f89,
            border: 0x363b54,
            accent: 0x7aa2f7,
            red: 0xf7768e,
            yellow: 0xe0af68,
            green: 0x9ece6a,
            blue: 0x7aa2f7,
            purple: 0xbb9af7,
            cyan: 0x7dcfff,
            orange: 0xff9e64,
            magenta: 0xf7768e,
        }),
    ),
    theme(
        "kanagawa-lotus",
        "Kanagawa Lotus",
        "Kanagawa",
        false,
        named_scheme(NamedScheme {
            background: 0xf2ecbc,
            surface: 0xe5ddb0,
            elevated_surface: 0xd5cea3,
            foreground: 0x545464,
            muted: 0x8a8980,
            border: 0x716e61,
            accent: 0x4d699b,
            red: 0xc84053,
            yellow: 0x77713f,
            green: 0x6f894e,
            blue: 0x4d699b,
            purple: 0x624c83,
            cyan: 0x597b75,
            orange: 0xcc6d00,
            magenta: 0xb35b79,
        }),
    ),
    theme(
        "kanagawa-wave",
        "Kanagawa Wave",
        "Kanagawa",
        true,
        named_scheme(NamedScheme {
            background: 0x1f1f28,
            surface: 0x2a2a37,
            elevated_surface: 0x363646,
            foreground: 0xdcd7ba,
            muted: 0x727169,
            border: 0x54546d,
            accent: 0x7e9cd8,
            red: 0xc34043,
            yellow: 0xe6c384,
            green: 0x98bb6c,
            blue: 0x7e9cd8,
            purple: 0x957fb8,
            cyan: 0x6a9589,
            orange: 0xffa066,
            magenta: 0xd27e99,
        }),
    ),
    theme(
        "everforest-light",
        "Everforest Light",
        "Everforest",
        false,
        named_scheme(NamedScheme {
            background: 0xfdf6e3,
            surface: 0xf4f0d9,
            elevated_surface: 0xefebd4,
            foreground: 0x5c6a72,
            muted: 0x829181,
            border: 0xe0dcc7,
            accent: 0x8da101,
            red: 0xf85552,
            yellow: 0xdfa000,
            green: 0x8da101,
            blue: 0x3a94c5,
            purple: 0xdf69ba,
            cyan: 0x35a77c,
            orange: 0xf57d26,
            magenta: 0xdf69ba,
        }),
    ),
    theme(
        "everforest-dark",
        "Everforest Dark",
        "Everforest",
        true,
        named_scheme(NamedScheme {
            background: 0x2d353b,
            surface: 0x343f44,
            elevated_surface: 0x3d484d,
            foreground: 0xd3c6aa,
            muted: 0x859289,
            border: 0x4f585e,
            accent: 0xa7c080,
            red: 0xe67e80,
            yellow: 0xdbbc7f,
            green: 0xa7c080,
            blue: 0x7fbbb3,
            purple: 0xd699b6,
            cyan: 0x83c092,
            orange: 0xe69875,
            magenta: 0xd699b6,
        }),
    ),
    theme(
        "ayu-light",
        "Ayu Light",
        "Ayu",
        false,
        named_scheme(NamedScheme {
            background: 0xfcfcfc,
            surface: 0xf8f9fa,
            elevated_surface: 0xffffff,
            foreground: 0x5c6166,
            muted: 0x828e9f,
            // Ayu's 12%-alpha `ui.line` composited over its editor background.
            border: 0xdfe3e8,
            accent: 0xf29718,
            red: 0xf07171,
            yellow: 0xeba400,
            green: 0x86b300,
            blue: 0x22a4e6,
            purple: 0xa37acc,
            cyan: 0x4cbf99,
            orange: 0xfa8532,
            magenta: 0xa37acc,
        }),
    ),
    theme(
        "ayu-dark",
        "Ayu Dark",
        "Ayu",
        true,
        named_scheme(NamedScheme {
            background: 0x10141c,
            surface: 0x0d1017,
            elevated_surface: 0x141821,
            foreground: 0xbfbdb6,
            muted: 0x5a6378,
            border: 0x1b1f29,
            accent: 0xe6b450,
            red: 0xf07178,
            yellow: 0xffb454,
            green: 0xaad94c,
            blue: 0x59c2ff,
            purple: 0xd2a6ff,
            cyan: 0x95e6cb,
            orange: 0xff8f40,
            magenta: 0xd2a6ff,
        }),
    ),
    theme(
        "flexoki-light",
        "Flexoki Light",
        "Flexoki",
        false,
        named_scheme(NamedScheme {
            background: 0xfffcf0,
            surface: 0xf2f0e5,
            elevated_surface: 0xfffcf0,
            foreground: 0x100f0f,
            muted: 0x6f6e69,
            border: 0xdad8ce,
            accent: 0x205ea6,
            red: 0xaf3029,
            yellow: 0xad8301,
            green: 0x66800b,
            blue: 0x205ea6,
            purple: 0x5e409d,
            cyan: 0x24837b,
            orange: 0xbc5215,
            magenta: 0xa02f6f,
        }),
    ),
    theme(
        "flexoki-dark",
        "Flexoki Dark",
        "Flexoki",
        true,
        named_scheme(NamedScheme {
            background: 0x100f0f,
            surface: 0x1c1b1a,
            elevated_surface: 0x282726,
            foreground: 0xcecdc3,
            muted: 0x878580,
            border: 0x403e3c,
            accent: 0x4385be,
            red: 0xd14d41,
            yellow: 0xd0a215,
            green: 0x879a39,
            blue: 0x4385be,
            purple: 0x8b7ec8,
            cyan: 0x3aa99f,
            orange: 0xda702c,
            magenta: 0xce5d97,
        }),
    ),
    theme(
        "dracula-alucard",
        "Dracula Alucard",
        "Dracula",
        false,
        named_scheme(NamedScheme {
            background: 0xfffbeb,
            surface: 0xefeddc,
            elevated_surface: 0xece9df,
            foreground: 0x1f1f1f,
            muted: 0x6c664b,
            border: 0xceccc0,
            accent: 0x644ac9,
            red: 0xcb3a2a,
            yellow: 0x846e15,
            green: 0x14710a,
            blue: 0x644ac9,
            purple: 0x644ac9,
            cyan: 0x036a96,
            orange: 0xa34d14,
            magenta: 0xa3144d,
        }),
    ),
    theme(
        "dracula",
        "Dracula",
        "Dracula",
        true,
        named_scheme(NamedScheme {
            background: 0x282a36,
            surface: 0x343746,
            elevated_surface: 0x424450,
            foreground: 0xf8f8f2,
            muted: 0x6272a4,
            border: 0x44475a,
            accent: 0xbd93f9,
            red: 0xff5555,
            yellow: 0xf1fa8c,
            green: 0x50fa7b,
            blue: 0xbd93f9,
            purple: 0xbd93f9,
            cyan: 0x8be9fd,
            orange: 0xffb86c,
            magenta: 0xff79c6,
        }),
    ),
];

pub fn all() -> &'static [BuiltinTheme] {
    THEMES
}

pub fn find(id: &str) -> Option<&'static BuiltinTheme> {
    all().iter().find(|theme| theme.id == id)
}

pub fn for_mode(dark_mode: bool) -> impl Iterator<Item = &'static BuiltinTheme> {
    THEMES
        .iter()
        .filter(move |theme| theme.dark_mode == dark_mode)
}

pub fn default_for_mode(dark_mode: bool) -> &'static BuiltinTheme {
    let id = if dark_mode {
        "tiptop-dark"
    } else {
        "tiptop-light"
    };
    find(id).expect("the built-in defaults are part of the static catalogue")
}

fn syntect_theme(theme: BuiltinTheme) -> Theme {
    let colors = theme.palette;
    let settings = ThemeSettings {
        foreground: Some(colors.foreground.into()),
        background: Some(colors.editor_background.into()),
        caret: Some(colors.caret.into()),
        line_highlight: Some(colors.current_line.into()),
        misspelling: Some(colors.error.into()),
        accent: Some(colors.accent.into()),
        gutter: Some(colors.gutter_background.into()),
        gutter_foreground: Some(colors.gutter_foreground.into()),
        selection: Some(colors.selection.into()),
        selection_foreground: Some(colors.selection_foreground.into()),
        ..Default::default()
    };
    let scopes = [
        ("comment", colors.comment, Some(FontStyle::ITALIC)),
        ("keyword, storage", colors.keyword, None),
        ("keyword.operator", colors.operator, None),
        ("string", colors.string, None),
        ("constant.numeric", colors.number, None),
        ("constant.language, constant.character", colors.label, None),
        ("entity.name.function, support.function", colors.link, None),
        ("entity.name.type, support.type", colors.heading, None),
        ("variable.parameter", colors.interpolated, None),
        ("markup.heading", colors.heading, Some(FontStyle::BOLD)),
        ("markup.italic", colors.emphasis, Some(FontStyle::ITALIC)),
        (
            "markup.underline.link",
            colors.link,
            Some(FontStyle::UNDERLINE),
        ),
        ("invalid", colors.error, None),
    ]
    .into_iter()
    .filter_map(|(scope, foreground, font_style)| {
        Some(ThemeItem {
            scope: ScopeSelectors::from_str(scope).ok()?,
            style: StyleModifier {
                foreground: Some(foreground.into()),
                background: None,
                font_style,
            },
        })
    })
    .collect();

    Theme {
        name: Some(theme.name.to_owned()),
        author: Some("tiptoptyp".to_owned()),
        settings,
        scopes,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn identifiers_and_names_are_unique_and_ids_are_stable_kebab_case() {
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for theme in all() {
            assert!(ids.insert(theme.id));
            assert!(names.insert(theme.name));
            assert!(
                theme
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            );
        }
    }

    #[test]
    fn catalogue_has_separate_light_and_dark_choices_with_paired_families() {
        assert!(for_mode(false).count() >= 5);
        assert!(for_mode(true).count() >= 7);
        for family in ["Tiptop", "Paper", "Ocean", "Forest"] {
            let variants = all()
                .iter()
                .filter(|theme| theme.family == family)
                .collect::<Vec<_>>();
            assert_eq!(variants.len(), 2);
            assert!(variants.iter().any(|theme| theme.dark_mode));
            assert!(variants.iter().any(|theme| !theme.dark_mode));
        }
    }

    #[test]
    fn catppuccin_flavours_match_the_official_palette_and_semantic_mapping() {
        let expected = [
            (
                "catppuccin-latte",
                false,
                0xeff1f5,
                0x4c4f69,
                0x1e66f5,
                0x8839ef,
            ),
            (
                "catppuccin-frappe",
                true,
                0x303446,
                0xc6d0f5,
                0x8caaee,
                0xca9ee6,
            ),
            (
                "catppuccin-macchiato",
                true,
                0x24273a,
                0xcad3f5,
                0x8aadf4,
                0xc6a0f6,
            ),
            (
                "catppuccin-mocha",
                true,
                0x1e1e2e,
                0xcdd6f4,
                0x89b4fa,
                0xcba6f7,
            ),
        ];
        for (id, dark_mode, base, text, blue, mauve) in expected {
            let theme = find(id).expect("Catppuccin flavour is bundled");
            assert_eq!(theme.dark_mode, dark_mode);
            assert_eq!(theme.palette.background, rgb(base));
            assert_eq!(theme.palette.foreground, rgb(text));
            assert_eq!(theme.palette.accent, rgb(blue));
            assert_eq!(theme.palette.link, rgb(blue));
            assert_eq!(theme.palette.keyword, rgb(mauve));
        }
    }

    #[test]
    fn popular_theme_families_have_exactly_one_light_and_one_dark_variant() {
        for family in [
            "Solarized",
            "Gruvbox",
            "GitHub",
            "Rosé Pine",
            "Tokyo Night",
            "Kanagawa",
            "Everforest",
            "Ayu",
            "Flexoki",
            "Dracula",
        ] {
            let variants = all()
                .iter()
                .filter(|theme| theme.family == family)
                .collect::<Vec<_>>();
            assert_eq!(
                variants.len(),
                2,
                "{family} should have exactly two variants"
            );
            assert_eq!(
                variants.iter().filter(|theme| !theme.dark_mode).count(),
                1,
                "{family} should have exactly one light variant"
            );
            assert_eq!(
                variants.iter().filter(|theme| theme.dark_mode).count(),
                1,
                "{family} should have exactly one dark variant"
            );
        }
    }

    #[test]
    fn popular_themes_match_their_canonical_core_palettes() {
        // background, foreground, accent, red, yellow, green, blue, purple, cyan
        let expected = [
            (
                "solarized-light",
                false,
                [
                    0xfdf6e3, 0x586e75, 0x268bd2, 0xdc322f, 0xb58900, 0x859900, 0x268bd2, 0x6c71c4,
                    0x2aa198,
                ],
            ),
            (
                "solarized-dark",
                true,
                [
                    0x002b36, 0x839496, 0x268bd2, 0xdc322f, 0xb58900, 0x859900, 0x268bd2, 0x6c71c4,
                    0x2aa198,
                ],
            ),
            (
                "gruvbox-light",
                false,
                [
                    0xfbf1c7, 0x3c3836, 0xaf3a03, 0x9d0006, 0xb57614, 0x79740e, 0x076678, 0x8f3f71,
                    0x427b58,
                ],
            ),
            (
                "gruvbox-dark",
                true,
                [
                    0x282828, 0xebdbb2, 0xfe8019, 0xfb4934, 0xfabd2f, 0xb8bb26, 0x83a598, 0xd3869b,
                    0x8ec07c,
                ],
            ),
            (
                "github-light-default",
                false,
                [
                    0xffffff, 0x1f2328, 0x0969da, 0xcf222e, 0x9a6700, 0x1a7f37, 0x0969da, 0x8250df,
                    0x1b7c83,
                ],
            ),
            (
                "github-dark-default",
                true,
                [
                    0x0d1117, 0xe6edf3, 0x2f81f7, 0xf85149, 0xd29922, 0x3fb950, 0x58a6ff, 0xbc8cff,
                    0x39c5cf,
                ],
            ),
            (
                "rose-pine-dawn",
                false,
                [
                    0xfaf4ed, 0x464261, 0x907aa9, 0xb4637a, 0xea9d34, 0x6d8f89, 0x286983, 0x907aa9,
                    0x56949f,
                ],
            ),
            (
                "rose-pine",
                true,
                [
                    0x191724, 0xe0def4, 0xc4a7e7, 0xeb6f92, 0xf6c177, 0x95b1ac, 0x31748f, 0xc4a7e7,
                    0x9ccfd8,
                ],
            ),
            (
                "tokyo-night-light",
                false,
                [
                    0xe6e7ed, 0x343b58, 0x2959aa, 0x8c4351, 0x8f5e15, 0x385f0d, 0x2959aa, 0x5a3e8e,
                    0x0f4b6e,
                ],
            ),
            (
                "tokyo-night",
                true,
                [
                    0x1a1b26, 0xa9b1d6, 0x7aa2f7, 0xf7768e, 0xe0af68, 0x9ece6a, 0x7aa2f7, 0xbb9af7,
                    0x7dcfff,
                ],
            ),
            (
                "kanagawa-lotus",
                false,
                [
                    0xf2ecbc, 0x545464, 0x4d699b, 0xc84053, 0x77713f, 0x6f894e, 0x4d699b, 0x624c83,
                    0x597b75,
                ],
            ),
            (
                "kanagawa-wave",
                true,
                [
                    0x1f1f28, 0xdcd7ba, 0x7e9cd8, 0xc34043, 0xe6c384, 0x98bb6c, 0x7e9cd8, 0x957fb8,
                    0x6a9589,
                ],
            ),
            (
                "everforest-light",
                false,
                [
                    0xfdf6e3, 0x5c6a72, 0x8da101, 0xf85552, 0xdfa000, 0x8da101, 0x3a94c5, 0xdf69ba,
                    0x35a77c,
                ],
            ),
            (
                "everforest-dark",
                true,
                [
                    0x2d353b, 0xd3c6aa, 0xa7c080, 0xe67e80, 0xdbbc7f, 0xa7c080, 0x7fbbb3, 0xd699b6,
                    0x83c092,
                ],
            ),
            (
                "ayu-light",
                false,
                [
                    0xfcfcfc, 0x5c6166, 0xf29718, 0xf07171, 0xeba400, 0x86b300, 0x22a4e6, 0xa37acc,
                    0x4cbf99,
                ],
            ),
            (
                "ayu-dark",
                true,
                [
                    0x10141c, 0xbfbdb6, 0xe6b450, 0xf07178, 0xffb454, 0xaad94c, 0x59c2ff, 0xd2a6ff,
                    0x95e6cb,
                ],
            ),
            (
                "flexoki-light",
                false,
                [
                    0xfffcf0, 0x100f0f, 0x205ea6, 0xaf3029, 0xad8301, 0x66800b, 0x205ea6, 0x5e409d,
                    0x24837b,
                ],
            ),
            (
                "flexoki-dark",
                true,
                [
                    0x100f0f, 0xcecdc3, 0x4385be, 0xd14d41, 0xd0a215, 0x879a39, 0x4385be, 0x8b7ec8,
                    0x3aa99f,
                ],
            ),
            (
                "dracula-alucard",
                false,
                [
                    0xfffbeb, 0x1f1f1f, 0x644ac9, 0xcb3a2a, 0x846e15, 0x14710a, 0x644ac9, 0x644ac9,
                    0x036a96,
                ],
            ),
            (
                "dracula",
                true,
                [
                    0x282a36, 0xf8f8f2, 0xbd93f9, 0xff5555, 0xf1fa8c, 0x50fa7b, 0xbd93f9, 0xbd93f9,
                    0x8be9fd,
                ],
            ),
        ];

        for (id, dark_mode, colors) in expected {
            let theme = find(id).expect("popular theme is bundled");
            assert_eq!(theme.dark_mode, dark_mode);
            let actual = [
                theme.palette.background,
                theme.palette.foreground,
                theme.palette.accent,
                theme.palette.error,
                theme.palette.warning,
                theme.palette.success,
                theme.palette.link,
                theme.palette.keyword,
                theme.palette.info,
            ];
            for (slot, (actual, expected)) in actual.into_iter().zip(colors).enumerate() {
                assert_eq!(actual, rgb(expected), "{id} core palette slot {slot}");
            }
        }
    }

    #[test]
    fn primary_copy_is_readable_and_transient_tints_remain_translucent() {
        for theme in all() {
            let palette = theme.palette;
            let contrast = palette.foreground.contrast_ratio(palette.background);
            assert!(
                contrast >= 4.5,
                "{} primary foreground must meet WCAG AA normal-text contrast (4.5:1); got {contrast:.2}:1",
                theme.name,
            );
            assert_eq!(palette.background.a, 255);
            assert_eq!(palette.surface.a, 255);
            assert_eq!(palette.editor_background.a, 255);
            assert!(palette.current_line.a < 64);
            assert!(palette.selection.a >= 50 && palette.selection.a < 96);
            assert!(palette.error_background.a < 64);
        }
    }

    #[test]
    fn each_builtin_can_drive_syntect_without_a_second_palette() {
        for builtin in all() {
            let theme = builtin.syntect_theme();
            assert_eq!(theme.name.as_deref(), Some(builtin.name));
            assert_eq!(
                theme.settings.background,
                Some(builtin.palette.editor_background.into())
            );
            assert_eq!(
                theme.settings.foreground,
                Some(builtin.palette.foreground.into())
            );
            assert!(theme.scopes.len() >= 12);
        }
    }
}
