# TeX dollar notation (todo 169)

## Using the mode

The main title bar order is traffic lights, **ttt**, **File Edit View** |
file title | right-aligned **miTeX Find Pause Compile** |
**Settings Explorer Code Split Preview Problems**. The title truncates into the
space remaining after the controls. Existing compact labels remain at narrow
widths. Hiding title-bar menus in Settings still works; the ttt color picker is
retained immediately after the traffic lights.

Click **miTeX** in the document toolbar to toggle the current document's mode;
its selected state indicates that it is on. The button is hidden for non-Typst
files, native Typst equations, invalid syntax and incompatible renderer bindings.
It stays available while already active, including during incomplete edits.

Settings → Editor has a separate **Auto-enable miTeX for compatible documents**
preference (off by default). It applies when opening/creating documents and when
first enabling the preference for the current compatible document. Incompatible
files open normally without a mode error. Turning the preference off does not
disable an active document, and manual toolbar toggles do not change the
preference. Editing or unrelated settings broadcasts do not override manual off.
New documents with auto-enable selected start empty. Reload retains an active
mode if the reloaded source remains compatible.

Settings also stores the pinned MiTeX version (default 0.2.7); changing it applies
on the next enable, not by rewriting imports in an active document.

The editor displays TeX, but saves and sends ordinary Typst to the compiler and
Tinymist:

| Displayed source | Generated source |
| --- | --- |
| `$\alpha$` | `#mi("\\alpha")` |
| `$ \alpha $` | `#mitex(" \\alpha ")` |
| Dollar pair with leading/trailing newlines | Multiline `#mitex("…")` |

Like Typst, whitespace at **both** ends selects display math. A newline somewhere
inside the expression alone does not. Code-context expressions omit the markup
`#`. Missing renderer imports are inserted once, and mixed inline/block input
imports both `mi` and `mitex`. These are MiTeX's
[inline and block renderers](https://typst.app/universe/package/mitex/).

Native Typst equations prevent enabling:
Typst math contents cannot safely be interpreted as TeX. Removing or explicitly
converting those equations allows enabling without first saving. Revert restores
the actual saved bytes, leaving the mode if that baseline contains native math.

Inside dollar pairs, including an unfinished pair, syntax highlighting and local
TeX completion use TeX rules. Ordinary Typst, comments and literals outside them
keep their normal behavior. Enter inside an empty paired dollar expression uses
the existing indented math-block insertion. There is no additional language server.

## Persistence and coordinates

The document owns displayed text and its undo history. A checked, immutable
canonical snapshot owns generated Typst plus the matching document owner, epoch,
revision, and bidirectional byte map. Conversion is cached once per revision,
including failures; line indexes are built lazily and shared by that snapshot.

Save, autosave, private backing files, CLI compilation, Tinymist synchronization,
and project-index overrides consume canonical bytes. Git compares canonical
bytes and maps changed-line intervals back to the displayed gutter; chunk text
and line totals describe the saved-file diff. Index rows retain saved-file line
numbers and go through the same canonical-to-editor navigation adapter as file
links. Hover, completion, formatting, preview selection and diagnostics use the
appropriate scalar or UTF-16 coordinate map.

Mode changes preserve disk identity and unsaved work, remap the selection, clear
history in the old representation, invalidate derived editor data, and restart
document services. Save receipts cannot cross a document/mode change or mark newer
edits saved. Failed open/save/disable operations retain the buffer. New/open
preference activation happens before service startup, avoiding duplicate jobs.

Untitled CLI compilation uses the real workspace source directory, not the
parent of the synthetic preview identifier. This fixes preview-before-first-save
for both modes, with a focused regression. The QA fixture initializes the same
private backing/service lifecycle as a real New document.

Unfinished/invalid input remains editable and undoable, but cannot be saved or
sent to a service as native Typst math. The last good preview may remain visible
until input becomes valid. Service edits are preflighted and applied atomically.
Old snapshot responses and unversioned projected diagnostics are rejected.
Tinymist preview navigation has no document-version field: generation checks and
strict mapping protect the available coordinates, but cannot prove the age of
an individual click in an older rendered preview.

## Deliberate limits

- Simple markup `mi` and `mitex` calls project into dollar notation. Untouched
  calls retain exact original quoting/whitespace. Named/additional arguments,
  indentation-trimmed raw blocks, code-context calls and ambiguous nested dollars
  remain explicit rather than being rewritten destructively.
- Binding checks fail closed for shadowing, renamed/nested imports, mismatched
  package versions and other wildcard imports. No macro expansion or imported
  name resolution is attempted.
- The lexical scanner handles escaped dollars and TeX line comments, not custom
  catcodes/verbatim delimiters. Such constructs can remain explicit MiTeX calls.
- A newly inserted import becomes visible on reopen. A formatter/service edit
  that changes only hidden spelling is refused with guidance to disable the mode,
  because displayed-text undo could not restore that invisible change.
- CLI compilation of a designated main file still reads imported subfiles from
  disk; Tinymist receives their unsaved canonical source.

## Disabled-mode performance

With the mode off, editing, dirty checks, saves, hover/completion coordinates,
preview navigation, highlighting and Git retain their ordinary paths. They do
not invoke projection parsing, encoding or coordinate-index construction.
Ordinary saves do not even allocate a canonical snapshot/cache. Deterministic
tests perform 100 edit/save cycles and assert zero encodes and no projection cache.
Mode setters invalidate highlighting/completion only when the flag changes.

The toolbar's compatibility check reuses the editor's parsed syntax, caches its
boolean by document revision and configured package version, and constructs no
translated text, offset map, or completion index. A changed revision needs one
validation traversal; unchanged frames do not parse or rescan the document.

Optimized source-layer measurements, 2026-09-16: Apple M2 Max, macOS 14.6.1
(23G93), Darwin arm64, rustc 1.96.0 (ac68faa20); release, thin LTO, one codegen
unit. No viewport/theme applies to these microbenchmarks. Run commands:

```sh
cargo test --release --lib disabled_projection_measurement -- --ignored --nocapture
cargo test --release --lib projection_cache_measurement -- --ignored --nocapture
```

Disabled-mode fixture: 325,000 bytes, 5,000 Unicode comment lines, 10 warmup
edits, five rounds of 200 alternating edits + dirty checks + save preparation +
history clearing. Baseline is the original core document; comparison is the
disabled adapter in the same binary. Execution order alternates each round.

| Round | Core baseline, µs | Disabled adapter, µs |
| --- | ---: | ---: |
| 1 | 2,368 | 2,531 |
| 2 | 2,409 | 2,257 |
| 3 | 2,447 | 2,254 |
| 4 | 2,237 | 2,264 |
| 5 | 2,396 | 2,257 |

These short runs show no consistent disabled-mode penalty, not a claimed speedup
or a cross-platform zero-overhead guarantee. The structural no-work assertions
are the stable regression protection.

Enabled-mode fixture: 325,062 bytes (same comment lines plus an imported MiTeX
call), 10 warmups, 200 iterations. Unchanged uncached encoding took 5,248 µs total;
cached snapshots 3 µs; cached position queries 8 µs. The separate alternating-edit
cache-miss workload took 345,672 µs (about 1.73 ms/edit), with exactly 201 encodes.
The earlier same-machine/profile fixture run took 335,765 µs (about 1.68 ms/edit);
this does not establish a meaningful change. Cache-hit timings must not be
compared with active-edit timings as a speedup. Full native interaction, cold
startup, and service-edit preflight latency are not covered by these measurements.

## Original mode validation (todo 169)

Regression coverage includes inline/display conversion, exact round trips,
imports, native-math refusal, Unicode maps, mapped Git lines and index navigation,
save/autosave/backing bytes, stale receipts, incomplete-input undo, atomic service
edits, diagnostics, local completion, highlighted byte ranges/cache reuse,
Settings transitions, and a semantic toolbar click test.

The opt-in application integration test saves generated source, compiles a PDF
with the installed Typst, reopens it, and observes a successful compile from the
real Tinymist server:

```sh
cargo test --bin tiptoptyp projected_application_compiles_real -- --ignored --nocapture
```

The deterministic `mitex-dollars` scene covers the active toolbar and inline/
display highlighting. `settings-editor` covers the compact controls at 360 points.
Fresh capture paths and final validation results are recorded below.

Passed: `cargo fmt --all -- --check`, strict all-target Clippy, the complete
ordinary test suite (58 library + 734 application tests, plus core/integration/
doc tests), and all 13 xtask tests. The opt-in real Typst/Tinymist integration
probe and both optimized measurements also passed.

Fresh final viewport PNGs inspected under `.tiptoptyp/screenshots/agent-review-169-verified/`:

- `1789569640077-0001-main-mitex-dollars.png` — light active-mode toolbar and math.
- `1789569649422-0001-main-mitex-dollars.png` — dark active-mode toolbar and math.
- `1789569649766-0002-settings-settings-editor.png` — 360-point Settings controls.

Checked text/highlight alignment, control sizing and clipping. These are egui
viewport framebuffers, not evidence of composed native child-view geometry;
this change does not alter those bounds.
Both the single-scene and serial captures exited successfully without a save
prompt. The final untitled captures no longer report the missing source directory.

The maintained gallery was regenerated in one app session and both capture-time
and standalone validation passed for all 68 PNGs. The dedicated mode fixture
also has a regression asserting it is clean and repeatable, so automated
screenshot exit cannot be intercepted by the unsaved-document prompt.

Measurement provenance: dirty checkout based on
`8fa854eb64404f0716c0491b1072674155eef0e3`. Source SHA-256:

```text
9bea5b7a889c30eacf5cfbc89420327802a34201af95650d4dfd455fb20f8723  src/mitex_document.rs
71bc6ae7eb07d756cec38d99722b15a2f6bcf66df25258ae993fc1a2222cb5a4  src/mitex_document/coordinates.rs
b5452413e407e3d798ae3d142d6cf8e215811688a9a61f131b7bf5b7c97c9c87  src/mitex_document/tests.rs
43b3357de1305312c7380ad3a3ae2d28699421ed8c05f2fc00855cb7b9358eb1  src/mitex_projection.rs
cab55e7ba70d7f97a414fd47b8cebab49cee72af23f84f0555b3b8f296b395f0  core/src/document.rs
```

## Title-bar and automatic activation follow-up (todo 173)

Semantic tests verify control order, non-overlap and right alignment at 680,
900 and 1,200 points, independent manual/automatic activation, normal opening
of incompatible files, and availability during incomplete active-mode edits.
Cache tests require only one compatibility validation per source revision or
package-version change, reuse parsed syntax, and never build the TeX completion
index for eligibility checks.

Optimized matched measurement on 2026-09-16, on the same Apple M2 Max / macOS
14.6.1 / rustc 1.96.0 environment and release profile described above:

```sh
cargo test --release --bin tiptoptyp profile_mitex_compatibility_cache -- --ignored --nocapture
```

The fixture is 325,001 bytes of Unicode comments plus an alternating final
character. After 10 warmup revisions, 200 changed revisions alternate execution
order between the existing source/syntax preparation baseline and preparation
plus compatibility validation. Snapshot creation is outside both timers.
Baseline total: 417,464 µs; with validation: 460,264 µs, an additional 214 µs
per changed revision in this run (about 10% of this syntax-only workload).
Separately, 20,000 unchanged cached checks took 200 µs total. There is no
per-frame parsing or projection construction. This is a source-layer
microbenchmark, not GUI frame latency, cold-start timing or a cross-platform
guarantee; no viewport/theme applies. Exactly 210 validations and equal syntax
rebuild counts are asserted, without wall-time thresholds.

Measurement checkout: dirty base `8fa854eb64404f0716c0491b1072674155eef0e3`.
Source SHA-256:

```text
e8ee1516d571b8f67aa13ef4a2dfa1bcaec9d8aa8f52674e61f96c580bcdaf3c  src/editor_data.rs
03d0acd27785103a576fb8a05c2a6301fe4215b2508dbd62fc56d854fe9bdf8c  src/mitex_projection.rs
```

Passed strict all-target Clippy, the complete ordinary test suite (59 library
and 737 application tests plus supporting suites), all 13 xtask tests, and the
optimized measurement. Fresh viewport framebuffers inspected under
`.tiptoptyp/screenshots/agent-review-toolbar/`:

- `1789573105973-0001-main.png`: reordered toolbar, miTeX hidden for incompatible source.
- `1789573106073-0002-main-mitex-dollars.png`: active miTeX, light theme.
- `1789573106190-0003-main-mitex-dollars.png`: active miTeX, dark theme.
- `1789573106313-0004-settings-settings-editor.png`: auto-enable control in narrow Settings.

The controls and separators are aligned, right-hand controls are right-aligned,
and Settings text fits without clipping. Native traffic lights are not part of
the egui framebuffer; the toolbar retains their existing reserved space. Native
child-view bounds are unchanged and composed desktop geometry was not verified.

Formatting and diff-whitespace checks also passed. The maintained gallery was
regenerated in one app session; capture-time and standalone validation both
passed for all 68 PNGs.
