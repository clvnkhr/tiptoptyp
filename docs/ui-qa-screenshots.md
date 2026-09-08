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
| `file-menu` | `popup` | File menu |
| `edit-menu` | `popup` | Edit menu and shortcut alignment |
| `settings-window` | `settings` | Settings child window |
| `settings-theme-picker` | `settings` | Open light-theme picker |
| `settings-dark-theme-picker` | `settings` | Open dark-theme picker |
| `settings-tooltip` | `settings` | Settings-local hover card |
| `typst-overrides-window` | `typst-overrides` | Independent light/dark Typst syntax overrides and live samples |
| `diagnostic-tooltip` | `diagnostic` | Diagnostic hover card |
| `function-tooltip` | `diagnostic` | Tinymist function-hover card |
| `save-dialog` | `modal` | Unsaved-changes card with save/discard choices |
| `alert-dialog` | `modal` | Informational or error alert card |
| `overwrite-dialog` | `modal` | Existing-file overwrite confirmation card |
| `editor-context-menu` | `popup` | Editor right-click menu |
| `explorer-context-menu` | `popup` | Explorer right-click menu |
| `document-font-selector` | `popup` | Scrollable font selector for a `text(font: …)` argument |
| `status-log` | `popup` | Recent compiler and document-status history |
| `rename-dialog` | `rename` | Rename card |
| `workspace-chooser` | `workspace` | Explicit workspace-root chooser |
| `problems-panel` | `main` | Expanded Problems panel with diagnostics |
| `find-replace` | `main` | Find and replace controls |
| `preview-compiling` | `main` | Compiling status and empty preview state |

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

Run the maintained light/dark matrix with:

```sh
scripts/capture-theme-gallery.sh
```

It captures every bundled theme in the ready main split view. It also captures
the maintained themed scene set in representative Catppuccin Latte and
Catppuccin Mocha matrices, including the error workbench and compiling preview
states, plus transformed main and elevated-menu examples to verify that the
ordered color operations reach every viewport. The complete checked-in matrix
is 68 decoded PNGs. The remaining deterministic scenes are still available for
targeted local captures and become gallery slots when their images are
committed. It uses `docs/ui-snapshots/theme-fixture.typ` by default; pass a
fixture path as the first argument to override it. For a quicker review, use
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
