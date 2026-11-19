# TeX and Typst editor feature coverage

The editor shares interaction rules across both languages, with language-aware
syntax services underneath. TeX support includes `.tex`, `.sty`, `.cls`, `.ltx`
and `.latex`, case-insensitively. Unsaved TeX tabs retain TeX highlighting and
editing behavior before their first save. Select the main document for preview,
or use an explicit `% !TeX root` comment, when editing a partial source file.

| Feature | Typst | TeX |
|---|---|---|
| Matching delimiters | Syntax-tree delimiters, math, strings and raw fences; the innermost enclosing pair stays highlighted while editing | Groups, brackets, math markers, sized/named math delimiters, mixed intervals and named environment pairs; comments and literal payloads are excluded. Both languages use a background highlight without an outline |
| Rainbow brackets | Shared family palettes and nesting colors | Same preferences and palettes, including math intervals and full environment commands; cached syntax highlighting is retained when palettes change |
| Automatic pairing | Language-aware markup, code and math delimiters | Braces, parentheses, square brackets, math dollars, `\[...\]` and `\(...\)`; second dollar expands an empty inline pair into a display pair. Enter after `\begin{env}` inserts an indented body and matching `\end{env}` unless already present |
| Selection surround, closer skipping and paired deletion | Supported with atomic Undo/Redo | Same editing guarantees; explicit selection wrapping stays shared, while empty-caret pairing uses TeX delimiter rules |
| Folding | Document headings and lexical scopes | Sections, environments, groups, display math and multiline command definitions; the same gutter, shortcuts and palette actions |
| Sticky context | Heading ancestry and enclosing lexical scopes | Section ancestry and enclosing environments/definitions; the same setting and navigation behavior |
| Web links | Literal `#link` calls | Literal `\url` and `\href` targets, including escaped punctuation; context menu and command-click use the same HTTP/HTTPS policy |
| Image/PDF hover | Literal image/include calls | Literal `\includegraphics`/`\includepdf` arguments, with options and unique extensionless image files; candidates remain inside the workspace |
| Local-use and custom text highlights | Syntax-aware identifiers and shared custom rules | TeX macro uses and the same custom rules |
| Indentation, guides and newline editing | Syntax-aware blocks, math and content | TeX groups, environments and math, using the same indentation preference and Undo/Redo rules |
| Find/replace, tabs, Git and ordinary editing | Shared editor | Shared editor, including paired typing in the editable Git diff view |
| Snippets, templates and tables | Typst syntax and imports | TeX syntax, magic-comment snippets, packages and booktabs/table editing |
| Math conversion | Right-click offers the other inline/display form directly | The conversion submenu offers `\(...\)` for display input or `\[...\]` for inline input, plus equation/alignment environments. Immediate sentence punctuation moves inside the chosen display form; whitespace, quotes and closing brackets stop capture. Each conversion is one Undo |
| Completion, hover, definitions and diagnostics | Tinymist plus local snippets/providers | Texlab, tex-fmt and TeX lint services plus local snippets/providers |
| Grammar, diagnostic fixes and comment controls | Shared settings and review flows | Same controls, with TeX-aware prose/math exclusions and `%` comment markers |
| Project contents and included files | Imports/includes, symbols, labels/references, packages and bibliography files | Inputs/includes/subfiles, sections/macros, labels/references, packages and bibliography files |
| Bibliography discovery | Literal bibliography strings/arrays, including BibTeX and YAML | BibTeX lists and biblatex resources, relative to their declaring source/import context; explicit magic roots and unsaved parent declarations are respected |
| Citation import | Review and append to a bibliography; optionally insert a citation | Same review flow; child documents can use an inherited project database, and an open `.bib` file is the preferred destination |
| Build, PDF preview and navigation | Typst compiler/PDFium or Tinymist interactive preview | Selected TeX engine/PDFium with SyncTeX source/preview navigation |

Provider-specific behavior remains with its provider. Tinymist's interactive web
preview, Typst font-argument chooser and MiTeX projection are Typst features;
TeX uses its engine settings, TeXdoc, packages and SyncTeX instead. Grammar and
compiler diagnostics retain their own language-specific messages.

Structural editing and project discovery are static analysis: they do not expand
TeX macros, compile source on the UI thread, or execute configuration text.
Dynamic bibliography expressions produce partial-index warnings rather than
guessed dependencies. Bibliography databases are leaf dependencies and never
contribute document headings or recursively execute source-like contents.
Asset hovers avoid guessing between multiple candidate files or expanding
graphics search-path macros.

Revision caches hold TeX delimiter/context/link indexes. Cursor queries use
prepared offsets; palette changes reuse the syntax cache. Asset resolutions are
cached until the source, target position, path or workspace changes, so idle
hover does not add filesystem reads. These rules have deterministic regression
coverage; opt-in component probes measure the explicit feature overhead and
cache work without making a cross-platform performance claim.

Enclosing matches in both languages use a revision-cached pair index. Warm
caret queries use bounded interval lookup, and stationary queries reuse their
last result; edits rebuild the affected source revision's index. The
[query-work record](performance-results/state-machines-delimiter-queries-2026-11-18.json)
separates rebuilds, stationary queries and moving queries. Highlight coordinates
reuse the existing galley row index instead of scanning preceding rows for each
endpoint glyph. In the same optimized 20,002-row component fixture, the median
pair-coordinate query changed from 273 µs to 0.918 µs with identical rectangles;
the [measurement](performance-results/enclosing-pair-coordinates-2026-11-18.json)
excludes parsing, layout, index construction and native rendering.

Local component measurements are retained for [pairing](performance-results/tex-editor-pairing-2026-11-18.json),
[bibliography indexing](performance-results/tex-bibliography-index-2026-11-18.json),
and [highlighting/asset caching](performance-results/tex-editor-caches-2026-11-18.json).
On macOS arm64 in the same optimized development test build, a 5,000-line
highlighting fixture took 6.1 ms per warm edit with rainbow disabled and 14.7 ms
with it enabled. A stable asset target took a median 53.9 µs per uncached query
and 0.082 µs from the cache. These are bounded component measurements, excluding
native interaction latency, cold startup and idle rendering; they establish no
cross-platform timing guarantee. Repeated idle queries are covered by deterministic
cache-build and filesystem-resolution counts rather than timing thresholds.
