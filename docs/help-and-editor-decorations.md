# Help and editor decorations

Open **Settings → ?** for a compact help page. It shows ten useful application
shortcuts and the preview shortcuts using the effective platform bindings,
including custom overrides and unbound actions. It includes tool-specific
formatter/lint directives, links to upstream instructions, and practical tips.
The help page belongs to Settings: closing Settings closes Help, and Find returns
to the settings search. The JSON button returns to the JSON editor.

Under **Settings → Editor**:

- **Wrap lines** toggles soft wrapping without rewriting source. The existing
  Toggle line wrapping action is available in View and Keyboard shortcuts.
- **Indentation guides** is on by default. **Guide character** accepts one
  visible character, such as `│` or `┆`. Guides use the configured space indent,
  or four columns when Tab inserts tab characters. Wrapped continuations,
  folded rows and non-leading whitespace are not decorated.
- **Highlight definitions and redefinitions** is optional. It emphasizes local
  Typst binding names, assignments and set targets, and TeX macro/environment
  declaration names. This is a source decoration, not execution or cross-file
  semantic resolution; dynamic TeX expansion is not evaluated.
- **Custom text highlights** contains literal, case-sensitive text matches and
  per-row foreground, background, weight, italic, underline and strike controls,
  with a live sample. TODO/FIXME have accent, bold, underlined defaults.
  Add a rule such as `#mycomment` or `// TODO:` to make your own constructs
  prominent. Disable or remove a row to stop matching it. Empty matches do
  nothing. Later matches take priority over earlier matches on overlap.

All options persist through the same settings update path and are editable in
validated JSON. Highlight rules are limited to 100 entries with 256 characters
per single-line match; guide characters and weight ranges are validated.
Source, character offsets, undo history, folding and search are unchanged.
Search highlighting is painted after custom highlighting so matches stay visible.

Decorations are applied when the existing syntax layout cache rebuilds; idle
frames reuse the decorated job. Match boundaries are swept in order rather
than comparing every syntax section with every match. Guide painting visits
only visible rows and bounds horizontal work to the clip rectangle.

# Tooltip sizing

Native text/Markdown hints now measure their visible contents, including code
fonts and headings, instead of enforcing a 280-point minimum width or reserving
an unused title row. Link targets do not contribute to width. Long documentation
still wraps and scrolls within the viewport cap, and the current size is cached
until content, fonts, style or available width changes. Settings-local hints and
ordinary egui hints already size to text; image/PDF hovers retain their fitted
image geometry.

# Checks

`editor_preferences` is the native journey for Help, settings search, guide,
definition and wrap toggles, source preservation and close/focus routing.
`controls` additionally verifies short native tooltip width, captures its own
focused viewport when review is requested, and exercises preview navigation,
dragging and repeated Outline transitions for both preview backends.

```sh
python3 scripts/test-desktop-ui.py --journey editor_preferences --capture-review
python3 scripts/test-desktop-ui.py --journey controls --capture-review
```

The optimized layout-cache measurement is opt-in:

```sh
cargo test --release --bin tiptoptyp decoration_cost_measurement -- \
  --ignored --nocapture --test-threads=1
```
