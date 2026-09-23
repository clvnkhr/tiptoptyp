# App-only UI screenshots

tiptoptyp's UI QA captures use egui's viewport framebuffer. They capture one
tiptoptyp window only; they never invoke a desktop or operating-system screen
capture. A capture of `main` does not include a sibling native child viewport,
so these PNGs are viewport evidence rather than a composed desktop screenshot.
Use the native geometry trace, and an approved whole-window observation when
available, to verify relationships between those surfaces.

Press `Cmd+Shift+F12` on macOS or `Ctrl+Shift+F12` elsewhere to capture the
focused supported window. PNG files are written to:

```text
<working-directory>/.tiptoptyp/screenshots
```

Launch-time captures can be named for repeatable manual checks:

```sh
cargo run -- --ui-screenshot main:split-light test.typ
cargo run -- --ui-screenshot settings:settings-dark test.typ
cargo run -- --ui-screenshot workspace:chooser
```

The part before `:` selects a viewport (`main`, `settings`, or the launch-time
`workspace` chooser); the part after it labels the PNG. Multiple
`--ui-screenshot` options may be supplied. Useful configuration options are:

```text
--ui-screenshot-settle 4
--ui-screenshot-shortcut cmd+shift+f11
--ui-screenshot-shortcut off
--ui-screenshot-subdir screenshots/narrow
--no-ui-screenshots
```

## Deterministic QA scenes

Use `--ui-snapshot-scene` to put the app into a typed, repeatable state before
capturing it. For example, this captures the actual transparent popup viewport
that carries the File menu above the preview:

```sh
cargo run -- \
  --ui-theme catppuccin-mocha \
  --ui-snapshot-scene file-menu \
  test.typ
```

When no explicit `--ui-screenshot` is supplied, a scene queues its canonical
target and name automatically. An explicit capture remains available for
special dimensions or labels.

The accepted scenes and their framebuffer targets are:

| Scene | Capture target | Themed state |
| --- | --- | --- |
| `main` | `main` | Main split editor and preview |
| `table-editor` | `table-editor` | Resizable table workbench with a merged heading, styling inspector and read-only source |
| `table-editor-narrow` | `table-editor` | The table workbench at its minimum width |
| `tabs` | `main` | Adjacent title-bar tabs with close controls, a dirty active tab, and the first tab selected as preview source |
| `empty-workspace` | `main` | No document tabs; workspace Explorer and New/Open remain available |
| `tabs-pdf` | `main` | PDF tab in the editor pane beside the first tab's Typst preview |
| `tabs-image` | `main` | Image tab in the editor pane beside the first tab's Typst preview |
| `window-color` | `logo-color` | Per-window logo background picker |
| `rainbow-brackets` | `main` | Independent bracket-family color cycles, nesting, and literal exclusions |
| `bracket-settings` | `settings` | Automatic pairing and per-family rainbow palette controls |
| `delimiter-match` | `main` | Matching delimiters at the caret without relaying out the source |
| `sticky-context` | `main` | Scrolled source editor with stacked headings and a multiline function header pinned independently of the caret |
| `folding` | `main` | Collapsed Typst, Markdown and TeX regions alongside expanded siblings and Git markers, using the unchanged gutter width |
| `mitex-dollars` | `main` | Active TeX mode toolbar control and inline/display TeX highlighting |
| `file-menu` | `popup` | File menu |
| `edit-menu` | `popup` | Edit menu and shortcut alignment |
| `settings-window` | `settings` | Settings child window |
| `settings-colors` | `settings` | 360-point Settings window with the unified color adjustments and reset |
| `settings-editor` | `settings` | 360-point Settings window scrolled to auto-save delay, with the label and slider kept together |
| `settings-status` | `settings` | 360-point Settings window scrolled to wrapping toolchain status chips |
| `settings-theme-picker` | `settings` | Open light-theme picker |
| `settings-dark-theme-picker` | `settings` | Open dark-theme picker |
| `settings-tooltip` | `settings` | Content-sized Settings-local luminosity hint |
| `typst-overrides-window` | `typst-overrides` | Independent light/dark Typst syntax overrides and live samples |
| `diagnostic-tooltip` | `diagnostic` | Diagnostic hover card |
| `function-tooltip` | `diagnostic` | Tinymist function-hover card |
| `save-dialog` | `modal` | Unsaved-changes card with save/discard choices |
| `alert-dialog` | `modal` | Informational or error alert card |
| `overwrite-dialog` | `modal` | Existing-file overwrite confirmation card |
| `editor-context-menu` | `popup` | Editor right-click menu |
| `explorer-context-menu` | `popup` | Explorer right-click menu |
| `unicode-completion` | `main` | Hebrew, mathematical, and alchemical glyphs in symbol completions |
| `font-completion` | `main` | Fuzzy font completion with a lazily loaded font sample |
| `settings-font-picker` | `settings` | Searchable font picker and the same live font sample used in hover cards |
| `git-editor` | `main` | Default-open Git section, badges, line totals, and gutter markers |
| `git-chunk` | `popup` | Selected chunk compared with the last commit, with stage/unstage/revert and navigation buttons and shortcut labels |
| `git-panel` | `main` | Repository status in the real Explorer Git subpanel using an in-memory fixture |
| `asset-preview` | `asset-hover` | Image-only asset card |
| `document-font-selector` | `popup` | Scrollable font selector for a `text(font: …)` argument |
| `status-log` | `popup` | Recent compiler and document-status history |
| `rename-dialog` | `rename` | Rename card |
| `workspace-chooser` | `workspace` | Explicit workspace-root chooser |
| `terminal-maximized` | `main` | Terminal filling available content height, with restore control |
| `explorer-maximized` | `main` | Files section filling Explorer, with sibling sections hidden |
| `terminal-panel` | `main` | Shared bottom panel with deterministic libghostty colors, Unicode and cursor; no shell is spawned |
| `problems-panel` | `main` | Expanded Problems panel with diagnostics |
| `find-replace` | `main` | Find and replace controls |
| `find-sticky-context` | `main` | Find/replace above scrolled sticky rows, with the current match count |
| `preview-compiling` | `main` | Compiling status and empty preview state |

A capture batch retains its initial document separately from scene-specific
buffers. Returning from a scene that replaces the document restores that fixture
and discards incompatible preview artifacts. A light/dark appearance change
finishes its current root frame before a new native child is created; existing
child windows keep their geometry and focus. This avoids creating a native GL
surface during the same appearance transition.

Unknown scene names are rejected rather than silently producing a mislabeled
image. `TIPTOPTYP_UI_SNAPSHOT_SCENE` is the environment equivalent. OS-native
file and folder pickers are outside the app framebuffer and therefore cannot be
included; `save-dialog` covers the custom themed modal shown around a save.
Together, `save-dialog`, `alert-dialog`, and `overwrite-dialog` cover every
app-owned modal layout.

Native tooltip cards remain connected to their source while the pointer crosses
the bridge. Click the card, or press Cmd+Shift+Space while its hover content is
available, to focus it for scrolling and text selection; Escape or defocusing
the card dismisses it.

## Theme gallery

Theme QA can override the persisted appearance without changing it:

```sh
cargo run -- \
  --ui-theme catppuccin-mocha \
  --ui-theme-invert \
  --ui-theme-hue-shift 30 \
  --ui-snapshot-scene main \
  test.typ
```

Inversion is applied first, followed by the hue shift. Hue shifts use whole
degrees in the range `-180..=180`. The equivalent environment variables are
`TIPTOPTYP_UI_THEME`, `TIPTOPTYP_UI_THEME_INVERT`, and
`TIPTOPTYP_UI_THEME_HUE_SHIFT`.

Use `--ui-screenshot-latest` together with an explicit `--ui-theme` for
reviewable snapshots. Requiring the theme identifier prevents a persisted
appearance from being saved under a misleading filename. Unlike ordinary
local captures, this mode writes directly to the checked-in gallery:

```text
docs/ui-snapshots/latest
```

Its filenames are stable and contain both the viewport and complete theme
profile. Scene captures also contain the validated scene name, for example:

```text
main--catppuccin-mocha.png
main--catppuccin-latte-inverted-hue-p30.png
popup-file-menu--catppuccin-mocha.png
diagnostic-diagnostic-tooltip--catppuccin-latte.png
settings-settings-window--catppuccin-mocha.png
```

Re-running the same capture replaces its previous gallery slot. Add
`--ui-screenshot-exit` to close tiptoptyp once all requested viewports have
been written. These options are also available as
`TIPTOPTYP_UI_SCREENSHOT_LATEST` and `TIPTOPTYP_UI_SCREENSHOT_EXIT`.

Run the maintained gallery with:

```sh
scripts/capture-theme-gallery.sh
```

The default is 22 PNGs: the main split view and all maintained component scenes
in Catppuccin Latte, plus three Catppuccin Mocha samples—the main window, File
dropdown, and Save dialog popup. Transformed variants are excluded by default;
set `TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS=0` to request them explicitly.
Existing images remain untouched until the next successful capture run prunes
the obsolete slots. The remaining deterministic scenes are still available for
targeted local captures and become gallery slots when their images are
committed and their manifest role is promoted from `targeted` to `component`.
`docs/ui-snapshots/gallery-manifest.tsv` is the shared source of truth for
theme order, scene targets, stable filename stems, pixel policies, and
transformed variants. It uses `docs/ui-snapshots/theme-fixture.typ` by default;
pass a fixture path as the first argument to override it. For a quicker review,
use
space-separated `TIPTOPTYP_UI_GALLERY_THEMES`,
`TIPTOPTYP_UI_GALLERY_SCENE_THEMES`, or `TIPTOPTYP_UI_GALLERY_SCENES`
overrides. The script launches only tiptoptyp's framebuffer capture path; it
does not call `screencapture` or any other desktop capture API.

Every capture command must succeed and produce its expected non-empty,
decodable PNG. A stale copy cannot mask a missed capture. Obsolete PNG slots
are pruned only after the entire default matrix succeeds; subset runs never
prune. Use `--print-manifest` to inspect the requested stable filenames or
`--validate-latest` to validate the checked-in matrix without launching the
app. With ImageMagick, validation also checks all four outer corners of every
elevated app viewport for transparency and catches pixel-identical Settings
window, theme-picker, and tooltip states without depending on fixed image
hashes.

The script builds the release app once, then runs that exact binary for the
entire matrix in one app session. It sends repeated
`--ui-screenshot-step theme,scene,invert,hue-shift` options to a serial runner;
each scene is reset, themed, settled, and written before the next step starts.
The whole session has a 45-second base watchdog plus one second per requested
image, so a missing child viewport cannot hang the gallery indefinitely.
Override the base with an integer from 1 to 3600 in
`TIPTOPTYP_UI_GALLERY_CAPTURE_TIMEOUT_SECONDS`; any capture or validation
failure restores every previous stable image before the script exits.

The corresponding environment variables are
`TIPTOPTYP_UI_SCREENSHOTS`, `TIPTOPTYP_UI_SCREENSHOTS_ENABLED`,
`TIPTOPTYP_UI_SCREENSHOT_SETTLE_FRAMES`,
`TIPTOPTYP_UI_SCREENSHOT_SHORTCUT`, and
`TIPTOPTYP_UI_SCREENSHOT_SUBDIR`, plus the scene variable
`TIPTOPTYP_UI_SNAPSHOT_SCENE`. Custom output subdirectories are constrained to
`.tiptoptyp`; only the fixed latest-gallery mode writes into `docs/`.

The `templates` and `encoding-import` scenes capture their own child viewport;
`writing-checks` captures the root editor with invisible/confusable character
markers. These targeted scenes supplement the maintained gallery.
