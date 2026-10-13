# Find overlay and writing-check audit (24 September 2026)

Find/replace used a foreground egui Area constrained to the source column.
That cannot cover the native embedded preview. Native runs now use the shared
child-window host with a modeless popup: fixed bounds, transparent corners,
explicit user-requested activation, and no dismissal merely because focus
returns to the editor. Bounds are limited by the document window rather than the
editor/preview divider. Embedded integrations retain the foreground Area.

When the owning document and its children lose focus, the native surface hides
without losing its query; it does not float above another app.

The child renders controls and returns intents; search navigation, replacements,
and undo snapshots run in the document owner's context. The child is included
in that owner's shortcut routing. Escape and close restore owner/editor focus.
The sizing cache requests a parent repaint only when measured height changes;
there is no new worker or periodic repaint loop. This changes composition, not
an algorithm requiring a throughput benchmark.

Regression coverage includes overflow across a narrow editor boundary, owner
bounds, modeless native policy, query input in a real child-context simulation,
and Escape restoring editor focus. Existing semantic find/refocus tests remain.
The two find screenshot scenes now target the native `find-replace` viewport;
that framebuffer cannot prove the composed desktop's stacking by itself.

## Writing checks: original findings and choices

The initial investigation below predates the parser exclusions, language
settings and bundled mathematical vocabulary. Typst and TeX now share a
[reviewed math supplement](../assets/dictionaries/README.md) alongside Harper's
curated dictionary. It accepts mathematical terminology and contributes spelling
suggestions while keeping ordinary spelling and grammar rules enabled.

`src/writing.rs` already uses `harper_typst::Typst` and `harper_tex::TeX`.
It applies Harper's curated dictionary/rules with British English and has no
project/user dictionary or per-rule suppression interface. Its existing math
regression checks only simple dollar-delimited expressions. The pinned Typst
adapter parses string literals as prose and special-cases selected function
arguments; general configuration strings can therefore be checked as English.
A proper name absent from the curated dictionary needs dictionary/ignore support,
not simply another markup parser. Exact problematic display-math forms should
be retained as fixtures rather than assuming all math modes fail identically.

Options:

1. Keep Harper and harden its integration: source-range exclusion for math/code
   strings, targeted parser fixes, project/user accepted-word lists, and separate
   spelling/grammar controls. Smallest incremental change, but custom macros may
   still need explicit configuration.
2. Own a shared prose extraction layer with source mappings. Use Typst syntax
   and conservative TeX environment/argument rules to admit prose; configure
   custom macros. Run Harper (or other checkers) only on admitted spans. More
   initial work, but consistent treatment of prose, strings, equations, and future
   writing services. Preserve rule IDs and spans to support useful suppression.
3. Add an optional LTeX+/LanguageTool service. Its official documentation covers
   offline checking, dictionaries, and language-server integration. This adds a
   service/runtime and larger downloads; keep it optional given the app-size goal.
   See https://ltex-plus.github.io/ltex-plus/ and
   https://ltex-plus.github.io/ltex-plus/vscode-ltex-plus/installation-usage-vscode-ltex-plus.html.
4. Offer a conservative mode immediately: spelling off, selected grammar rules
   retained, full checks on demand. Less noise but also missed real misspellings;
   filtering and accepted-word lists are still desirable.

Preferred sequence: option 1 for immediate relief, with option 2 as the common
architecture if robust Typst/TeX support is the priority. Do not blanket-ignore
capitalized words; it would hide genuine mistakes at sentence starts.

The full gallery also exposed an existing serial-capture startup ordering bug:
the first capture was queued after document construction, so the document could
start with the interactive renderer while the gallery waited for PDFium readiness.
The first batch request is now queued before constructing the document.

## Validation

Formatting, strict Clippy, all 1,239 tests, and all 15 xtask tests passed. The full
23-image gallery was regenerated in one app session and `--validate-latest`
passed. The three dark captures and remaining light captures retain the existing
policy. Inspected both the gallery's native find card and the fresh optimized
framebuffer at
`.tiptoptyp/screenshots/find-overlay/1790242518695-0001-find-replace.png`.
It is 1240 × 166 pixels, with readable controls and rounded transparent margins.
The desktop observation tool captured the owner separately, so combined native
window stacking was not visually verified; ownership/geometry/native input
regressions and the shared popup window policy cover it structurally.
Logs and the retained `ui.preview.bounds` trace are in `.tiptoptyp/find-overlay/`.


## Mathematical prose boundary audit (8 October 2026)

Harper's pinned adapters discarded punctuation inside equations. TeX's upstream
mask also converted a hidden single newline plus indentation into a paragraph,
splitting a correct sentence around display math or a formatting command. Typst's
post-lint equation exclusion then removed genuine sentence-wide warnings.

The language-specific prose adapters now retain terminal top-level commas,
semicolons, colons and periods at their original Unicode-scalar positions, while
keeping each equation an opaque semantic object. A display equation does not
start a paragraph by itself. Actual blank lines, headings, list boundaries and
explicit TeX paragraph commands still separate prose. Math labels and closing
syntax after terminal punctuation do not hide the next sentence's first word.
Sentence-initial spelling mistakes after equation punctuation remain eligible;
the existing proper-name heuristic applies only within a sentence.

The adapters conservatively leave decimal points, tuples, nested matrix content,
ASCII ellipses, factorials, question-mark notation and unfinished equations
opaque. TeX `\text{...}` annotations remain checked, while literal code payloads
remain excluded. Typst technical named arguments are excluded before grammar
runs, rather than discarding any resulting lint that overlaps an equation. The
Typst adapter also repairs upstream argument-token ordering only when necessary.
No macro expansion or TeX execution is used for writing checks.

[Reviewed mathematical vocabulary](../assets/dictionaries/README.md) now includes
whole hyphenated compounds and additional operator/PDE terms. Unknown spelling,
repeated words and capitalization rules remain enabled; the generic spelling
message has not been hidden or treated as a separate class of error.

All parsing remains on the existing delayed writing worker, with its existing
size limit and request coalescing. There is no new per-frame parsing, worker,
repaint timer, or UI-thread dictionary construction. Compound recognition uses
one in-place pass with bounded lookups and retains token capacity; deterministic
regressions cover long unknown chains and source gaps. These changes improve
correctness and make no claim of faster grammar processing.

The checked-in `manual-tests/harper-math-prose.typ` and `.tex` fixtures cover
punctuation both inside and outside display math, aligned TeX, indented formatting,
ordinary prose, compounds and names. The `harper_math_prose` desktop journey opens
both real files, verifies a completed clean result, types a spelling mistake and
repeated word, verifies their exact original locations and file paths, and then
checks stale-result retention followed by a fresh clean result. Completion is
observed against the exact document revision and settings, not inferred from
an idle previous result. This is semantic/native interaction coverage, not
composed visual evidence; no UI pixels or layout contract changed.


Validation for this audit: all 1,730 executed Rust tests, 17 xtask tests and
45 desktop-harness contract tests pass. Formatting and strict all-target Clippy
pass both normally and with `desktop-ui-tests`. The focused prose/worker suite
and malformed-TeX scanner regressions pass after recorded failing reproductions.

The attempted native command was
`caffeinate -diu -t900 python3 scripts/test-desktop-ui.py --journey harper_math_prose --release`.
It stopped at preflight because the Mac was locked (`desktop: false`,
`locked: true`), with Accessibility and input permissions already enabled.
No app interaction was executed, so native coverage remains unverified. The
attempt's evidence directory is `.tiptoptyp/desktop-ui-tests/run-y7vf7t93`.
