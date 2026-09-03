# Bundled theme sources

tiptoptyp maps established upstream palettes into one shared set of semantic UI
and syntax roles. Neutral ramps supply window, panel, elevated, border, and text
colors; upstream accents supply diagnostics and syntax. Palette colors are kept
verbatim except where noted, while translucent selection and diagnostic fills
are generated consistently by tiptoptyp.

| Family | Bundled light theme | Bundled dark theme | Canonical palette source |
|---|---|---|---|
| Solarized | Solarized Light | Solarized Dark | [altercation/solarized](https://github.com/altercation/solarized#the-values) |
| Gruvbox | Gruvbox Light | Gruvbox Dark | [morhetz/gruvbox](https://github.com/morhetz/gruvbox/blob/master/colors/gruvbox.vim) |
| GitHub | GitHub Light Default | GitHub Dark Default | [primer/github-vscode-theme](https://github.com/primer/github-vscode-theme/blob/main/src/theme.js) |
| Rosé Pine | Rosé Pine Dawn | Rosé Pine | [rose-pine/palette](https://github.com/rose-pine/palette/blob/main/palette.json), [rose-pine/neovim palette](https://github.com/rose-pine/neovim/blob/main/lua/rose-pine/palette.lua) |
| Tokyo Night | Tokyo Night Light | Tokyo Night | [tokyo-night/tokyo-night-vscode-theme](https://github.com/tokyo-night/tokyo-night-vscode-theme/tree/master/themes) |
| Kanagawa | Kanagawa Lotus | Kanagawa Wave | [rebelot/kanagawa.nvim](https://github.com/rebelot/kanagawa.nvim/blob/master/lua/kanagawa/colors.lua) |
| Everforest | Everforest Light | Everforest Dark | [sainnhe/everforest](https://github.com/sainnhe/everforest/blob/master/palette.md) |
| Ayu | Ayu Light | Ayu Dark | [ayu-theme/ayu-colors](https://github.com/ayu-theme/ayu-colors/tree/master/themes) |
| Flexoki | Flexoki Light | Flexoki Dark | [Flexoki by Steph Ango](https://stephango.com/flexoki), [kepano/flexoki palette](https://github.com/kepano/flexoki/blob/main/README.md#colors) |
| Dracula | Dracula Alucard | Dracula | [dracula/dracula-theme](https://github.com/dracula/dracula-theme#color-palette-oss) |

The existing Catppuccin variants come from the
[official Catppuccin palette](https://github.com/catppuccin/palette/blob/main/palette.json)
and follow its [editor style guide](https://github.com/catppuccin/catppuccin/blob/main/docs/style-guide.md).
Tiptop, Paper, Ocean, and Forest are native tiptoptyp palettes.

Solarized intentionally uses selective contrast. Solarized Light uses its
canonical darker `base01` for primary application text instead of historical
body `base00`, keeping normal UI copy above the 4.5:1 accessibility threshold
without changing its hues. All other listed core colors use their upstream hex
values directly.
