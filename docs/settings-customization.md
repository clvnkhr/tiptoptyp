# Settings customization

Settings uses compact categories: Appearance, Editor, Writing, Preview, Tools,
Shortcuts, Snippets and Templates. Wide windows show the categories beside the
form; narrow windows use a selector next to search. Search spans every category.
Choose a result to switch to its category and briefly highlight its control.
Switching categories retains the query, scroll positions and unfinished drafts.
It does not resize the window or apply unsaved JSON/tool configuration.

Appearance keeps color adjustments collapsed until opened or reached by search.
Its page sample uses the same theme, page mode and independent comfy background
and text choices as the actual preview; it does not compile a document.

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
| Tectonic | `-X compile --untrusted --keep-logs --keep-intermediates` |
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
server. Each has Apply and Reset. TexLab defaults leave compilation to Tectonic
and formatting/linting to Badness, with a 300 ms diagnostic delay and fuzzy
case-insensitive completion. Badness shows its existing fallback defaults,
`{"lineWidth":80,"indentWidth":2}`. These affect formatting unless a discovered
configuration file supplies the project style. Tool commands and server
configuration are separate settings.

Under Tinymist, **Tinymist configuration (JSON)** exposes formatter choice,
width, indentation, prose wrapping, input variables, lint policy and export policy.
The app retains the preview connection and designated entry. `typstExtraArgs`
accepts only `--input=KEY=VALUE` options. Lint stays disabled; its trigger is
ready for `onType` because this client does not send explicit save notifications.

Open Settings → **? Help & tips** → **Tool options** for ten useful choices
per bundled tool, copyable examples, defaults, tradeoffs and official references. Badness LSP formatting/lint options mostly
belong in `badness.toml`; `--config` and `--no-config` do not affect this mode.

**Ignored diagnostic codes** accepts comma-separated codes, for example
`redundant-script-braces`. No codes are ignored by default. The Problems label
`Badness · Code: "redundant-script-braces"` describes the diagnostic; it is not
an argument to pass to the binary. Search Settings for “ignored codes” to find it.

## English proofreading

Choose **Auto**, **English (UK)**, or **English (US)**. Auto reads literal,
unconditional top-level Typst `#set text(lang: "en", region: "us")` settings;
`region: "gb"` selects UK. No language/region setting and TeX documents fall back
to UK. Explicit UK/US overrides document settings. An explicitly non-English
Typst language disables English checking in Auto. `lang: "uk"` means Ukrainian,
not UK English. Computed values, imports and locally scoped language changes are
not evaluated by this offline detector.

## Diagnostic lifetime contract

Editing, scheduling, restarting or failing a checker does not remove its last
published diagnostics. A successful result, including an empty result, replaces
them. Changing documents or explicitly disabling a provider retires its results.
Providers must reject obsolete responses before publication, and must not treat
“work pending” or an infrastructure failure as a successful empty result. This
contract applies to new adapters as well as Harper, Tinymist and TeX services.

## Custom snippets and linked fields

Settings → Snippets opens the snippet source form. Search Settings for **snippets** or **completion** to open it directly. Choose **Add Typst snippet** or **Add TeX snippet**, edit the prefix and description, and write ordinary source in the highlighted editor with line numbers and rainbow brackets. Pick Typst, TeX or Both, then **Save snippets**. Invalid drafts cannot be saved and do not change live completions; **Revert changes** restores the saved list. Each language starts with an editable example.

Backslashes, dollar math, quotes and line breaks are literal: do not JSON-escape them. Optional editable fields use `${1:name}`; repeat `${1}` to mirror the same field, and use `${0}` for the final cursor. Bare `$1`, `$0` and `$x$` are ordinary source. For example, a TeX snippet with prefix `env`:

```tex
\begin{${1:enumerate}}
  ${0}
\end{${1}}
```

Type the prefix and accept its completion. Tab/Shift+Tab navigate fields, Escape ends linked editing, and undo/redo apply to mirrored copies together. Editing outside the selected field or switching documents ends the session. Numbered defaults and nested fields are supported; transformations and variables are not interpreted in custom source. Server-provided snippets continue to use standard LSP syntax.

Autocomplete uses a link for a language server, a document for a custom snippet and code brackets for a built-in command. Font suggestions show only their name, rendered in that font. Completion rows have no explanatory tooltips. Filtering and rebasing preserve each candidate's provider.

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


## PDF and image appearance

Appearance → Page chooses Light, Dark, or Follow interface for PDF previews.
Comfy background and Comfy text independently match paper and ink to the app.
Images preserve their original colors by default. Enable **Dark images** to invert
image colors when the Page choice is dark; Light pages show original colors.
The image header's contrast icon toggles this same preference. This affects the
preview only: source image files and transparency remain unchanged.

## LaTeXdiff

**Edit → LaTeXdiff…** opens an owned modal above the document, including native
previews. Choose an older and newer `.tex` file. When the newer file is the
current editor document, **Include unsaved editor changes** compares a snapshot
against the older file on disk. By default both paths are the current file, so
this compares the last saved version against the editor.

**Generate diff** runs LaTeXdiff in the background. Choose underlined, coloured,
or traditional markup; **Include subfiles** expands `\input` and `\include`.
**Copy source** copies the result; **Save and open…** saves a separate `.tex`,
opens it in a new tab, and selects it for the existing TeX/PDF preview pipeline.
Save beside the newer document to preserve relative graphics/bibliography paths.
Neither original may be used as the output path. Esc or Cmd+W closes the modal;
closing cancels generation and removes its temporary snapshot. A confirmed save
is completed even if its initiating window closes.

LaTeXdiff is discovered in PATH or MacTeX's `/Library/TeX/texbin`; it is not
bundled. **LaTeXdiff options**, also under Settings → TeX tools, provides its
executable, literal arguments, JSON environment and working directory. For
example `--math-markup=1` marks whole equations. All options are available in
settings JSON at `latexdiff`; invalid markup choices and malformed command
settings are rejected. See the [official manual](https://mirrors.ctan.org/support/latexdiff/doc/latexdiff-man.pdf).
