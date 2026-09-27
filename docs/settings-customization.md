# Settings customization

The **{}** button at the top-right of the Settings title bar toggles between
the form and JSON. Settings can be maximized and restored normally.
JSON starts with the complete current
settings, including defaults. Edits are validated as you type; **Save JSON
settings** applies valid edits through the same persistence path as the form.
**Reload current settings** discards the draft. Unknown app keys, wrong types,
invalid choices (with the field name and allowed options), invalid ranges,
malformed command arguments/environment, and invalid working
directories are rejected. Tool-specific configuration is an arbitrary JSON object:
the receiving tool validates its own keys. Editing a draft does not change the app.

## Tool commands

Each binary's **Command customization** section shows its effective customization.
The default is `{args}`, environment `{}`, and a blank working directory. These
mean generated arguments, inherited environment, and the operation's document or
project directory. Reset restores these defaults. Hover the field labels for
examples and links to that tool's documentation.

- Arguments use shell-style quoting only; there is no shell execution, `$HOME`
  expansion, or `~` expansion. `{args}` preserves generated arguments;
  `{arg:0}` selects the first generated argument. Removing `{args}` replaces them.
  For example, Typst: `{args} --font-path "/Users/me/My Fonts"`.
- Environment is a JSON object of strings, merged into the inherited environment.
  Example for a Rust tool supporting this variable: `{"RUST_LOG":"debug"}`.
- Working directory is blank or an existing absolute directory, such as
  `/Users/me/Documents/My Paper`. Do not surround this field with quotes.
  Changing it changes where the tool resolves relative paths.

Under **TeX tools**, expand **TexLab configuration (JSON)** or **Badness
configuration (JSON)** to see and override the actual configuration sent to the
server. Each has Apply and Reset. TexLab defaults leave compilation to Tectonic
and formatting/linting to Badness. Badness has no app-specific configuration
(default `{}`). Tool commands and server configuration are separate settings.

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

Settings → Editor → Custom snippets lets you add a prefix, language (`typst`, `tex`, or `both`), description, and snippet body. The same entries are editable in the `snippets` JSON array. For example:

```json
{
  "snippets": [{
    "prefix": "env",
    "description": "LaTeX environment",
    "language": "tex",
    "body": "\\begin{${1:enumerate}}\n  $0\n\\end{$1}"
  }]
}
```

Type the prefix and accept its completion. Numbered fields are selected for replacement; repeated field numbers mirror the edit. Tab/Shift+Tab navigate fields, `$0` finishes, and Escape ends linked editing. Undo/redo apply to both copies together. Editing outside the selected field or switching documents ends the session. Placeholder defaults and first-choice lists are supported; snippet transformations are not.

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
