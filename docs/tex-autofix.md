# TeX autofixing

Settings → Autofixing contains the TeX rule switches and Liquid diagnostic-fix
model controls. Known fixable diagnostics also offer a rule-specific action in
Problems; it checks all occurrences of that enabled rule in the current document.
**Autofix TeX Document** checks the current TeX document in one background
operation and applies its admitted edits as one undo step. It does not require
an up-to-date Problems panel. The action is available from the Edit menu; its
shortcut can be changed in Settings → Keyboard shortcuts.

**Autofix on save**, in the same Autofixing section, is off by default. Enabling
it applies the selected TeX rules when saving. Manual autofixing remains
available independently of this switch.
Saving a protected Overleaf draft as a local recovery copy preserves its exact
text and skips autofixing and formatting; resolving its conflict remains a
separate action. Saving a healthy parked tab does not edit the active tab.

Liquid suggestions continue to open a diff for Accept or Cancel. The bulk
autofix action uses the explicit rules below and never accepts model output.
It currently operates on TeX documents, not Typst or embedded TeX in Typst.

## Rule inventory

The provider catalog was checked against the **bundled Badness 0.29.0 binary**,
using `badness lint --no-config --explain <rule>` for all 39 LaTeX rule IDs and
`badness lint --no-config --output json -` on source fixtures. The
[upstream rule reference](https://badness.dev/reference/linter-rules) provides
additional examples. This inventory describes the pinned binary rather than
assuming that a future server has the same rules or fixes.

| Setting | Default | Correction and boundaries |
| --- | --- | --- |
| Non-breaking spaces before references | On | Replace the complete whitespace run before `\ref`, `\eqref` or `\pageref` with one `~`, including source line breaks. Preserve existing ties, explicit spaces, math, comments, literals, definitions, paths and locally redefined reference commands. |
| Non-breaking spaces before bracketed citations | Off | Accept Badness's `missing-nonbreaking-space` edits for bracketed citation commands; do not alter textual citations or self-describing references. |
| Blank lines inside display math | On | Insert `%` on an empty interior line in a structurally paired display, including `$$...$$` and nested formula groups. Keep indentation, line endings and line numbers. Unclosed displays and inline math stay untouched. |
| Blank lines in key-value arguments | On | Accept `blank-line-in-keyval` fixes only where the parser identifies a processor argument that cannot contain a paragraph. |
| Deprecated font commands | On | Accept `deprecated-command` control-word replacements where Badness has ruled out macro redefinitions and mere references. |
| Primitive subscript/superscript commands | On | Accept equivalent `primitive-command` token replacements such as `\sb` → `_`; argument-restructuring primitives remain manual. |
| Use `\[...\]` instead of `$$...$$` | On | Accept the paired `dollar-display-math` edits without changing the formula. |
| Ellipses in text | On | Accept `ellipsis` fixes classified safe by Badness, including command/letter separation. |
| Ellipses in math | Off | Allow `ellipsis` fixes classified unsafe: selecting centered/baseline dots changes mathematical typography. |
| Redundant script braces | Off | Accept `redundant-script-braces` only where the provider proves removing both braces preserves tokenization. |
| Spaces swallowed by TeX logo commands | Off | Accept `swallowed-space` insertion of `{}` after argument-free logo commands. Changes visible spacing. |
| Unwanted spaces before footnotes and labels | Off | Accept `space-before-command`; the provider decides whether removing the gap would join unrelated prose. |
| Spacing after abbreviations/acronyms | Off | Accept `abbreviation-spacing` fixes, which deliberately alter sentence/interword spacing. |
| Straight quotation marks | Off | Accept atomic `straight-quotes` replacements; quotation direction is inferred. |
| Dashes in number ranges | Off | Accept available `dash-length` fixes for numeric ranges; ambiguous word dashes stay manual. |
| Multiplication signs between numbers | Off | Accept `times-variable` edits; a variable named `x` can be intentional. |
| Backslashes before math operators | Off | Accept `math-operator-name` edits; a sequence such as `sin` can also be a product. |
| Extra final math line breaks | Off | Accept `extra-math-linebreak` deletion of an empty final row. Can affect spacing and equation numbering. |
| Labels before captions/list items | Off | Accept the atomic `label-before-caption` move only where Badness supplies one. Changes the referenced number. |
| Obsolete math environments | Off | Accept paired `obsolete-environment` renames. Check package requirements and alignment layout before enabling. |
| Obsolete Badness suppression comments | On | Accept `deprecated-suppression-syntax` edits within the directive while retaining its selector and reason. |

All switches are independent, persisted settings. Turning a rule off excludes
its candidates from the next action. Diagnostic visibility settings remain
separate: an ignored cosmetic diagnostic can still be fixed when its autofix
rule is enabled.

## Diagnostics that remain manual

The remaining LaTeX rule IDs were reviewed individually. These cases do not
have a single reliable automatic source replacement:

| Diagnostic | Reason to retain review |
| --- | --- |
| `duplicate-label` | Choose which definition to rename/delete and which references to retarget. |
| `expl3-invalid-message-parameter` | The intended message argument is unknown. |
| `expl3-protected-predicate` | Choosing protection or expandability changes the function API. |
| `expl3-variant-type` | The intended expl3 signature cannot be inferred. |
| `extra-alignment-tab` | Either a row or the column specification can be wrong. |
| `hard-coded-reference` | A correct local label or external reference is missing. |
| `indented-docstrip-guard` | Activating a guard can change generated package files. |
| `inert-suppression` | The intended suppression region/target is unknown. |
| `invalid-macrocode-frame` | A `.dtx`-specific fix; this action analyzes TeX document source, not documented package extraction. |
| `makeat-macro` | The correct scope for category-code changes is not a local textual choice. |
| `sectioning-level-jump` | The author must choose the document hierarchy. |
| `missing-required-argument` | The missing argument's content must be supplied. |
| `undefined-ref` | Requires a complete project namespace and the intended target. |
| `undefined-citation` | Requires bibliography resolution and the intended key. |
| `unreferenced-label` | Keeping the label or adding a reference can both be correct. |
| `verbatim-trailing-text` | Choose where to move the text or whether to delete it. |
| `duplicate-package` | Removing a load can discard necessary options. |
| `missing-provides` | Choose the package identity, placement, date and version. |
| `unknown-option` | Choose the correct option or whether it should be removed. |
| `unclosed-math-delimiter` | An opener can be stray, or a missing closer can belong at several positions. |
| `lonely-item` | The list type and boundaries are unknown. |

`primitive-command`, `dash-length` and other partly fixable rules keep their
report-only cases manual when the provider returns no edit. The BibTeX-only
rules are outside this TeX action's scope. TexLab also reports unresolved
references/citations and project issues. Build engines report missing packages,
undefined commands, package options, missing arguments/delimiters, alignment
errors, font/file problems and box warnings. Their prose messages do not grant
permission to invent source replacements. The specialized source rules can
resolve the corresponding known spacing/paragraph case; other errors retain
manual editing, provider actions or the reviewed Liquid suggestion flow.

## Ownership, protection and bounded work

`src/tex/autofix.rs` produces exact byte-range edits against an immutable source
snapshot. The app owns the request's document identity, settings and cancellation
handle. A newer document, workspace, source revision or relevant settings change
retires the request before its result can apply. Plans also verify the entire
source and each edit's original text. Invalid UTF-8 endpoints, unknown fix
applicability, overlapping candidates and partially protected multi-edit fixes
are rejected; a paired delimiter change or label move is admitted atomically.

Standalone diagnostics-off and combined-off markers protect their regions,
including custom spellings and whitespace variants. Control marker lines are
also protected. Formatting-only markers retain their existing formatter scope.
Shared TeX lexical indexing excludes comments and literal payloads; the one
explicit Badness-suppression rule can edit its recognized comment directive.

Source rules run first against the private snapshot; Badness then checks that
repaired snapshot once. Provider spans are mapped back through source edits,
rejecting corrections that cross changed text. This fixes a blank display
paragraph and converts its dollar delimiters in one action while retaining
granular cursor mapping and one undo step.

Badness runs for an explicit autofix action or an opted-in save, in an owned
process group, with a 30-second limit
and 64 MiB output limit. Its stdin/stdout snapshots live in an automatically
removed project-private `.tiptoptyp/autofix-*` directory. It receives
`--no-config`, explicit selected rules and a synthetic `.tex` stdin filename;
it never receives `--fix` or a writable user-file path. Source-only fixes remain
available without a working Badness process, with a message explaining any skipped
provider rules.
The implementation adds no idle scans, repaint loop or per-frame disk work.
