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

## Writing checks: findings and choices (not implemented)

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
