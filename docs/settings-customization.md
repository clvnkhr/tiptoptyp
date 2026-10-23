# Settings customization

Settings uses compact categories: Appearance, Editor, Grammar & spelling, Writing,
Preview, Tools, Shortcuts, Snippets and Templates. Wide windows show the categories beside the
form; narrow windows use a selector next to search. Search spans every category.
Choose a result to switch to its category and briefly highlight its control.
Switching categories retains the query, scroll positions and unfinished drafts. The Snippets category gives its saved library the available height, with per-row enable, Edit and Delete controls; Edit opens that entry in the separate source editor.
It does not resize the window or apply unsaved JSON/tool configuration.

Appearance keeps color adjustments collapsed until opened or reached by search.
Its page sample uses the same theme, page mode and independent comfy background
and text choices as the actual preview; it does not compile a document. The
sample figure also follows **Apply dark page appearance to images**.

**Compact layout controls** groups Code, Split, Preview, Explorer and Panel in
one Layout menu. Turn it off to restore their separate title-bar buttons.
The same choice is available as `compact_layout_controls` in JSON.

The **{}** button at the top-right of the Settings title bar toggles between
the form and JSON. Settings can be maximized and restored normally.
JSON starts with the complete current
settings, including defaults. Edits are validated as you type; **Save JSON
settings** applies valid edits through the same persistence path as the form.
**Reload current settings** discards the draft. Unknown app keys, wrong types,
invalid choices (with the field name and allowed options), invalid ranges,
malformed command arguments/environment, and invalid working
directories are rejected. Tool-specific configuration must be a JSON object. Known Tinymist choices and
types are checked before applying; app-owned preview keys and entry overrides
are rejected. Other provider keys are validated by the receiving tool. Editing
a draft does not change the app.

## Tool commands

Under **Tools**, each binary's **Command customization** section shows its actual default arguments,
editable directly. Environment starts as `{}` and working directory is blank,
meaning inherited environment and the operation's document/project directory.
Reset restores that tool's defaults. Hover field labels for examples and links.
JSON stores this literal argument list in `command.argv`; the old template field
has been removed.

| Tool | Editable default arguments |
|---|---|
| Typst | `watch --diagnostic-format short` |
| Tinymist | `lsp` |
| Tectonic | `-X --chatter minimal --color never compile --untrusted --keep-logs --keep-intermediates` |
| TexLab | Empty |
| Badness | `lsp` |
| tex-fmt | `--stdin --quiet` |
| LaTeXdiff | Empty; markup, UTF-8 encoding, input paths and optional `--flatten` are managed by the dialog |

- Arguments use shell-style quoting only; there is no shell execution, `$HOME`
  expansion, or `~` expansion. For example, Typst:
  `watch --diagnostic-format short --font-path "/Users/me/My Fonts"`.
  The app appends input/output paths, workspace roots, discovered fonts and
  flags controlled by SyncTeX/offline settings. Protocol-critical arguments
  are required, with a clear validation error if removed. Optional flags remain
  the binary's responsibility. Argument placeholders are not supported.
- Environment is a JSON object of strings, merged into the inherited environment.
  For Tectonic: `{"TECTONIC_CACHE_DIR":"/Users/me/tectonic-cache"}`.
- Working directory is blank or an existing absolute directory, such as
  `/Users/me/paper`. Do not surround this field with quotes.
  Changing it changes where the tool resolves relative paths.

Under **TeX tools**, expand **TexLab configuration (JSON)** or **Badness
configuration (JSON)** to see and override the actual configuration sent to the
server. Each has Apply and Reset. Reset also restores that provider's ignored diagnostic codes without changing other tools. TexLab defaults leave compilation to Tectonic
and formatting/linting to Badness, with a 300 ms diagnostic delay and fuzzy
case-insensitive completion. Badness shows its existing fallback defaults,
`{"lineWidth":80,"indentWidth":2}`. These affect formatting unless a discovered
configuration file supplies the project style. Tool commands and server
configuration are separate settings. TexLab defaults also hide exactly `Unused label`
and `Unused entry` hints through anchored `diagnostics.ignoredPatterns`; unresolved
and duplicate references/citations remain enabled. Its ignored-code list remains empty.

Tectonic progress output defaults to minimal, with plain text logs. The root options
`--chatter minimal --color never` must precede `compile`. **Show underfull box warnings**
defaults off; those messages remain in raw build output. Overfull boxes and compiler
failures remain visible. External latexmk defaults to `-quiet -bibtex-cond` and
BibTeX to `-terse`; final TeX and bibliography logs still supply Problems diagnostics.
Biber arguments stay empty. These external tools are discovered from the TeX installation.

Under Tinymist, **Tinymist configuration (JSON)** exposes formatter choice,
width, indentation, prose wrapping, input variables, lint policy and export policy.
The app retains the preview connection and designated entry. `typstExtraArgs`
accepts only `--input=KEY=VALUE` options. Lint stays disabled; its trigger is
ready for `onType` because this client does not send explicit save notifications.

Open Settings → **? Help & tips** → **Tool options** for ten useful choices
per bundled tool, copyable examples, defaults, tradeoffs and official references. Badness LSP formatting/lint options mostly
belong in `badness.toml`; `--config` and `--no-config` do not affect this mode.

**Ignored diagnostic codes** accepts comma-separated codes, for example
`redundant-script-braces`. Badness defaults hide twelve advisory rules: abbreviation spacing, display-math delimiter preference, ellipses, hard-coded references, ties before references, obsolete environments, primitives, redundant script braces, heading-level jumps, spacing before commands, straight quotes and unused labels. Remove a code to restore that report. The app filter affects Badness only and does not change project formatting or lint computation. The Problems label
`Badness · Code: "redundant-script-braces"` describes the diagnostic; it is not
an argument to pass to the binary. Search Settings for “ignored codes” to find it.

## Grammar & spelling

Settings → **Grammar & spelling** keeps **Harper spelling and grammar**,
**LTeX+ spelling and grammar**, **Liquid selection grammar**, and **TeX prose rules**
together. TeX prose rules apply to Harper and Liquid; LTeX+ uses its own LaTeX
configuration. Harper and LTeX+ remain independent background checkers, with
separate provider names in Problems. Liquid checks selected text only and opens a
correction review. Writing retains handwriting, punctuation, Unicode and math
formatting preferences.

Expand **Harper configuration** for **Harper language**, **Harper dictionary**,
**Harper rule configuration**, and **Harper: ignore capitalized names**. Harper rules
show the editable academic defaults, with syntax highlighting, validation, Apply and
Reset. Sentence-length, hedging, filler, discourse-marker, source-spacing, comma-style,
dash, ellipsis and abbreviation-expansion advice defaults off; spelling, repeated words,
a/an, agreement and apostrophe checks remain enabled. Unknown rule names and non-boolean
values cannot be saved. The Harper dictionary and name filter
affect Harper only. Choose **Auto**, **English (UK)**, or **English (US)**. Auto reads literal,
unconditional top-level Typst `#set text(lang: "en", region: "us")` settings;
`region: "gb"` selects UK. No language/region setting and TeX documents fall back
to UK. Explicit UK/US overrides document settings. An explicitly non-English
Typst language disables English checking in Auto. `lang: "uk"` means Ukrainian,
not UK English. Computed values, imports and locally scoped language changes are
not evaluated by this offline detector.

LTeX+ defaults disable source-whitespace, sentence/paragraph-length, repeated-opening,
plain-English rewriting, redundancy-style and dash-style advice. Its JSON explicitly
shows the language-specific disabled-rule lists, accepted mathematical vocabulary,
enabled rules and ignored findings. Picky rules remain off. **Follow document language**
uses the same document language policy as Harper; disable it to use the JSON `language`
value explicitly. UK English is the fallback. Accepted words affect spelling only.
Both background grammar providers remain off in factory defaults; existing user
preferences are not replaced. Liquid remains an explicit selected-text review.

## Diagnostic lifetime contract

Editing, scheduling, restarting or failing a checker does not remove its last
published diagnostics. A successful result, including an empty result, replaces
them. Changing documents or explicitly disabling a provider retires its results.
Providers must reject obsolete responses before publication, and must not treat
“work pending” or an infrastructure failure as a successful empty result. This
contract applies to new adapters as well as Harper, Tinymist and TeX services.

## Snippets and linked fields

Settings → Snippets lists every bundled suggestion and your own entries, using the available category height. Enable, Edit and Delete act on individual rows. **Edit** opens that entry in the separate source window; **Edit snippets…** opens the full editor for adding or restoring entries. Search and filter by language, origin and enabled state. The source editor retains highlighting, line numbers and rainbow brackets. Cmd/Ctrl+F searches the editor, Cmd/Ctrl+S saves valid changes, and Cmd/Ctrl+W closes only that window. Unsaved drafts survive closing and reopening.

Write ordinary Typst or TeX and use the same placeholder syntax in either language:

- `{{1:x}}` creates field 1 with initial text `x`.
- `{{1}}` repeats field 1; linked copies update together.
- `{{2:y}}` creates the next field.
- `{{0}}` is the final cursor position.

Numbers determine Tab order. Dollar signs, backslashes and document commands are ordinary source. For example:

```tex
\begin{{{1:equation}}}
  {{2:x = y}}
\end{{{1}}}
{{0}}
```

```typ
#let {{1:name}}({{2:x}}) = {{2}}{{0}}
```

Each snippet has **Opening delimiter** and **Closing delimiter**, defaulting to `{{` and `}}`. Choose another pair, such as `<<` and `>>`, when those characters clash with the source. **Rewrite markers** converts existing placeholders; **Keep source** changes the reserved delimiters without rewriting the body. Save stays disabled until this choice is made and the resulting definition is valid. The same syntax works for shared snippets; document language only determines where they are offered and how their source is highlighted.

The JSON fields are `placeholder_open` and `placeholder_close`. Bundled suggestions live in `snippet_presets`, custom entries in `snippets`. **Enabled** temporarily pauses an entry; **Duplicate** copies it; **Save snippets** applies the editor draft; **Revert changes** restores saved definitions. **Restore suggestions** resets the bundled list while keeping your own entries. An enabled, eligible custom entry overrides a suggestion with the same prefix. Invalid definitions cannot be saved even when disabled.

`scope` is `anywhere`, `text`, `math` or `code` (Typst code); `line_start` allows indentation. Comments and raw/verbatim text are excluded. Fractions and sums appear inside equations; `mk` starts inline math from text. Auto-miTeX dollar regions use TeX snippets. `fn` inserts a markup declaration beginning with `#let`; `fncode` inserts bare `let` inside a code block.

Type the prefix, open completions (Ctrl+Space by default), choose the snippet and press Enter. Tab/Shift+Tab navigate fields; Escape ends linked editing. Undo/redo applies to mirrored copies together. Multiline expansions retain indentation; recognized TeX package imports are inserted atomically. Editing outside the active field or switching documents ends linked editing. Keep editable fields flat rather than nesting them. Templates do not execute code, transformations or choice menus.

Completion symbols distinguish language-server, snippet and built-in entries. Fonts show only their name rendered in that font. A walkthrough and examples live in **Settings Help → Snippets**.

## TeX tools in auto-miTeX documents

`tex.embedded_diagnostics` and `tex.embedded_formatting` default to `true`. They independently enable TeX checking and the selected TeX formatter for projected dollar math. Tinymist continues serving the Typst document. TeX tools receive a private virtual document containing math regions; diagnostics map back into those regions, and formatter output replaces only the math payloads. Unrelated Typst bytes are preserved. A response with missing region boundaries is rejected instead of replacing the source.

## Draw symbol

Open Explorer → Draw symbol, draw with the pointer, then choose a result. Recognition is offline and runs after a stroke ends. Clear removes the drawing and results. The bundled Detypify model covers 411 symbol classes; TeX documents display available TeX aliases (390 classes), while Typst displays symbol names. Selecting a Typst result inserts its Unicode character; TeX inserts its command when available. Some TeX commands require packages in the document.

The drawing canvas fills the panel; resizing preserves its ink and stroke width.
Predictions appear as non-selectable text over subdued ink; click to insert.
Detexify's full accepted sample set supplies additional LaTeX commands, including
calligraphic letters (`\mathcal{A}` / Typst `cal(A)`). Script letters (`\mathscr{A}` / `scr(A)`), phonetic letters and many other
symbols also work in Typst. Unverified mappings are labelled as such and disabled
only for Typst insertion. See [recognition coverage](../assets/handwriting/README.md).


## Window background

On macOS, Appearance controls blur and transparency independently. Choose a
direction, then set Start and End for Blur radius and Background opacity.
The chosen edge uses End; the opposite edge uses Start. Increasing, decreasing,
and equal endpoints all work. Uniform uses Start everywhere. Opacity 100% is
opaque and 0% is fully transparent. Fullscreen disables both effects while
retaining the saved values; PDF page colours remain independent.

JSON uses `window_background.blur_radius` / `blur_end_radius` (0–50),
`window_background.opacity_percent` / `opacity_end_percent` (0–100), and the
independent `blur_fade` / `transparency_fade` directions: `none`, `left`, `right`,
`up`, or `down`. The defaults are blur 8→0 and opacity 80→100, with Uniform selected.

## PDF and image appearance

Appearance → Page chooses Light, Dark, or Follow interface for PDF previews.
Comfy background and Comfy text independently match paper and ink to the app.
Images preserve their original colors by default. Enable **Dark images** to invert
image colors when the Page choice is dark; Light pages show original colors.
The image header's contrast icon toggles this same preference. This affects the
preview only: source image files and transparency remain unchanged.

## LaTeXdiff

Use **File → Create LaTeX Diff…**, or search `latexdiff` in the command palette.
The wizard is available with any document open, or with no open tabs:

1. Choose the older and newer `.tex` files. The current saved TeX document fills
   the newer field; optionally include its unsaved editor changes.
2. Choose a new output `.tex` file and options. The suggested name is
   `NEWER-diff.tex` beside the newer file. Markup and subfile expansion have
   controls; **Arguments** passes literal options such as `--math-markup=1`.
   **Advanced** contains the executable, JSON environment and working directory.
3. Review the paths and create the diff. **Open diff** opens the result in a new
   editor tab and selects it for the normal TeX/PDF preview. **Copy source** is
   available after creation.

Generation, input checks and saving run on a background worker. **Cancel** stops
an active job; Esc, Cmd+W or closing the wizard also cancels it. Original files
and existing output files are never overwritten. Publication is atomic: canceled
or failed jobs leave no partial output. Save beside the newer document to retain
relative graphics/bibliography paths.

LaTeXdiff is discovered in PATH or MacTeX's `/Library/TeX/texbin`; it is not
bundled. Defaults are editable under Settings → Tools → TeX tools and in settings
JSON at `latexdiff`. Arguments use shell-style quoting to split literal values,
without shell expansion. Explicit arguments follow the preset options, so an
option such as `--type=CFONT` can override the selected markup. Input paths are
supplied by the app. Environment is a JSON object of strings, for example
`{"LC_ALL":"C"}`; blank working directory uses the newer file's directory.
See the [official manual](https://mirrors.ctan.org/support/latexdiff/doc/latexdiff-man.pdf).

## Optional Liquid grammar

Grammar & spelling → Liquid selection grammar configures a manual right-click action,
independently of Harper. Liquid runs in a separate native ONNX Runtime helper, without
Python, on Intel Macs with macOS 12 or newer and Apple Silicon Macs with macOS 14
or newer. `liquid_cache_directory` defaults to the persistent app-data folder
when empty, and `liquid_threads` defaults to 4 (valid range 1–8). A custom cache
folder must be absolute. Model loading and inference run outside the UI thread;
no model or inference library is loaded at app startup.

Select prose and choose **Check selection with Liquid**. On first use the optional
model and current Mac runtime are downloaded from the published
[`liquid-onnxruntime-v1` resource release](https://github.com/clvnkhr/tiptoptyp/releases/tag/liquid-onnxruntime-v1),
verified and prepared automatically. The default app bundle contains neither the
model nor its native runtime. Grammar & spelling
also offers **Download Liquid** to download or repair the local installation, with
Cancel while the task is active. The complete selection is shown before and after,
with changes highlighted using the Git diff style chosen in Settings. Apply is
one undoable operation. Checking is offline after setup; no selected text is sent
to the download server. Existing caches from bundled-model builds need a one-time download of about 362 KB of licence/provenance files; their verified model is reused. See Settings Help for the workflow and supported
runtime, or the [comparison and integration notes](grammar-comparison.md).

`liquid_input` controls projection before inference:

```json
{
  "protected_content": "placeholder",
  "placeholder": "x",
  "formatted_text": "include"
}
```

`protected_content` accepts `placeholder` or `split`. The first preserves sentence
context with a protected word; the second checks prose on either side of math,
code and links separately. `placeholder` accepts 1–24 ASCII letters.
`formatted_text` accepts `include` or `skip` for recognized formatting bodies
(`*text*`, `#emph[...]`, `#strong[...]`, `#text[...]`, TeX `\emph{...}`,
`\textbf{...}` and related standard commands). Skipped bodies become protected
content; commands are never sent verbatim. Unknown macros retain conservative
parser behavior. These options affect Liquid only, not Harper.

The review's **Liquid input** disclosure shows the exact prepared fragments.
These are the submitted fragments when inference runs; they remain inspectable
when a missing model prevents inference. TeX readable text inside supported
math-text commands can be admitted; Typst equations remain wholly protected.
Changed placeholders, boundary-crossing edits and source-syntax changes are
rejected rather than applied.
