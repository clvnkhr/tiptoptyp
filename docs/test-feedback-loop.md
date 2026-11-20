# Test feedback-loop audit — 20 November 2026

This audit inventories the Rust targets, script/CI lanes and all registered
native journeys. Changes concentrate on repeated setup, oversized fixtures,
fixed sleeps and duplicated responsibility. A typed phase does not prove that
an external reply belongs to it, or that the OS delivered an input event.
Stale-result, cancellation, owner-departure and real native-boundary evidence
therefore remain.

## Pick the owning target while iterating

Local execution is the primary validation path. GitHub mirrors the same runner;
no hosted run is required to use or verify it:

```sh
./scripts/check-local.sh fast      # complete script/browser lane; also the default
./scripts/check-local.sh rust      # the four required Rust/package checks
./scripts/check-local.sh features  # strict compilation + feature-owned contracts
./scripts/check-local.sh all       # all three local lanes, stopping on failure
```

Run with the repository's pinned Rust toolchain; the runner uses a locked
dependency graph. It resolves the repository from its own location, so it also works when
called from another directory. Native journeys remain explicit local commands
because they require a logged-in desktop and, for some cases, real sidecars or
optional models. Instrumentation changes still require the additional full
profiling checks specified in the working agreement.
Every supplemental feature owner must actually pass a test; stale filters and
zero-test selections fail the local gate. Dependency policy is also available
locally with `cargo deny check`.

```sh
# Document, text, identity and other core policies; no application test binary.
cargo test -p tiptoptyp-core

# Public library policies: themes, projection and save transactions.
cargo test --lib

# Application unit/semantic tests without the separate CLI executable build.
# Multiple libtest filters are ORed by the pinned Rust 1.98.1 harness.
cargo test --bin tiptoptyp -- tex:: editor_data::
cargo test --bin tiptoptyp -- writing::cache:: tinymist::tests::pending_deadlines

# One integration executable; filter its owning module when appropriate.
cargo test -p tiptoptyp --test contracts -- architecture_boundaries:: build_identity_inputs::

# No Rust build, models or desktop. Full scripts include private local servers.
python3 -m unittest discover -s scripts -p 'test_desktop_ui_*contract.py'
python3 -m unittest discover -s scripts -p 'test_*.py'

# One build/app session, retaining the normal shared-session order.
python3 scripts/test-desktop-ui.py --only dialogs --only images --only math_grids
```

`--only` selects leaves of the existing shared `all` session. It rejects unknown,
duplicate or focused-only journeys and cannot combine with `--start-at` or a
different `--journey`. Receipts record requested and executed selections;
omitted journeys never count as coverage. Model/tool-specific fixtures still
use their individual `--journey` entry point. `--executable` reuses an explicitly
preserved inspection-enabled binary and records its actual hash/version; it
does not establish source provenance or freshness on its own.

Run the complete [required checks](../AGENTS.md#required-checks-before-handoff)
before handoff. Focused commands accelerate iteration; they do not replace that
gate. Automatic macOS Cargo cache cleanup remains enabled and lock-safe.

## Changes and coverage ownership

| Previous work | Replacement | Evidence retained |
| --- | --- | --- |
| 18 separately linked ordinary integration targets, launched sequentially | One `tests/contracts.rs` executable containing the same modules | All API, CLI, archive, build-input, package, vendor-policy and gallery assertions; three AppKit/main-thread harnesses stay separate |
| Real Tinymist process waits three seconds merely to reach a deadline | Pure `PendingCall` construction/expiration tests at exact clock boundaries; already-expired real worker smoke | Earliest-deadline and tie ordering, completion/removal, reported failure, stopping and child reaping |
| Four real children for an encoding-value matrix | One real adapter rejection witness | Exhaustive malformed/default/UTF-16 matrix in `lsp::protocol::tests::position_encoding_defaults_to_utf16_and_rejects_other_or_malformed_choices` |
| TeX synchronization/provenance cases wait for ten fake child frames, then replace their mailboxes | Admit semantic messages directly into bounded mailboxes; one framed stdio handshake remains | Full/incremental edits, full-queue behavior, diagnostic clear, stale snapshots, document switches and actual initialize→Ready wiring; failed OS startup reports child status and bounded stderr |
| Lint 1,032 paragraphs twice just to exceed a cache cap | `ParagraphCache` owns bounds/accounting/LRU; cheap direct cap tests and one real grammar eviction/recompute/hit witness | Actual 1,024-entry and 8 MiB limits, collision checks, LRU promotion, replacement accounting, clear, zero-allocation oversized rejection and diagnostic equality |
| Repeated native Open/Save implementations and fixed 0.8 + 0.3 + 0.5 s pauses | One readiness-based native dialog protocol, exact loaded path and optional editor-focus admission | Native picker/button readiness and path ownership; assets retain their renderer-specific readiness checks |
| Three duplicate Open/Save protocol tests and a stale hard-coded journey count | Shared dialog protocol tests; selection/registration and feature assertions remain | Disabled Save, no premature typing, exact path/focus, asset focus exception, failure detection |
| Math-grid Cancel repeated for every kind | Cancel once per native dialog/language, using the same kind-independent handler | All six kind-specific Apply/Undo bindings and Typst resize remain; draft sessions fall from 13 to 9 |
| Comment-control Cartesian product repeated across providers | Default combined controls in both language backends, representative custom-channel isolation | Formatter cases 8→5, diagnostic cases 8→5, off/on publication phases 16→10; six custom Settings fields and both actual providers remain |
| Scripts/browser contracts wait behind Rust setup and repeat across Rust jobs | One local `fast` command owns the complete script/browser corpus; CI optionally mirrors it in an independent job | macOS-specific lock/deletion contracts additionally run on macOS; real tools/browser/native jobs remain separate |
| Default suite repeated in profiling mode plus a third desktop feature variant | Local `rust` runs the full default suite; `features` strictly compiles combined profiling/desktop code and runs its owned contracts | `performance::` includes recorder and dormant behavior; `desktop_test::tests` covers inspection/geometry; Linux retains its profiling variant |
| Formatting/policy waits for GUI SDK/package setup | These non-compiling admissions run before native build setup in CI | SDK/linking preflight still precedes every native compile, including delegated local-runner commands |

The shared dialog change removes a **19.2-second syntactic fixed-sleep budget
across 12 source sites**. Calls/loops can repeat those sites, and readiness still
takes real time. This is not a measured whole-suite or native speedup.
Measurements and their limits are recorded separately in the performance receipt.
The old dialog journey also depended on an optional macOS Search control and
failed that lookup in the retained before run. The shared Go-to-Folder protocol
tests native input delivery through its actual path-entry field and verifies the
exact opened document and returned keyboard ownership. The updated Open/Save
journey passed; a failed before run does not support an elapsed-time speedup claim.

The [measurement receipt](performance-results/test-feedback-loop-2026-11-20.json)
retains three interleaved AB/BA/AB rounds, binary/source hashes and raw artifact
locations. On the same M2 Max, Rust 1.98.1 dev/test opt-level 1 build:

| Fresh-process test workload | Before median | After median |
| --- | ---: | ---: |
| Tinymist unanswered-initialize worker smoke | 3.26 s | 0.24 s |
| Cache-bound plus grammar-equivalence witness | 2.44 s | 0.42 s |
| Installer + gallery integration cases | 9.26 s | 9.18 s |

The integration execution result is effectively unchanged. Its structural gain
is 18 ordinary targets/links/post-exit runner calls becoming one; the timing
above does not establish a build-time gain. These fresh-process measurements
exclude Cargo compilation, native journeys and whole-app latency. No builds or
profilers ran concurrently; ordinary host activity was uncontrolled.

Large application code generation remains a separate cost. Target selection
avoids building the CLI executable during app-unit iteration, and combined
supplemental features remove a redundant variant. A next measured experiment
could put Tract inference behind an opaque leaf-library API, reusing unchanged
model code across app/test/features. Moving the whole app into a library alone
would still compile normal and test variants. See the remaining build experiment
in [the performance workflow](performance.md#build-invalidation-and-tex-table-lookup-19-november-2026).

## Why retained checks still matter, and what could retire more

| Retained area | Present boundary | Better code that could reduce future testing |
| --- | --- | --- |
| Worker/service state machines | External results still need exact owner/request admission; cancellation must reap/drain resources | A reusable transport actor around `OwnedChild`, with one OS pipe/reaping smoke and a pure error/stale/recovery event matrix, could eliminate repeated process fixtures. Keep the event sequences. |
| Package/compiler-backed table journeys | Pure parsing cannot prove the compiler returns correct macro/variable samples | Parameterize compiler-worker integration tests for tabularray/siunitx and all supported table dialects. Then keep one native locked-cell/apply witness per language instead of repeating package loops in the GUI. |
| Comment markers and formatting | Shared parser/projection/admission own the combinatorics; native controls still bind both backend paths | The reduced native witnesses are sufficient while shared handlers remain single owners. New provider-specific behavior needs its own adapter test, not another full GUI matrix. |
| Native focus, window ownership, picker, clipboard and IME | AppKit/OS events and composition are outside Rust type guarantees | Centralize common platform adapters, then test each platform boundary once. Pure text/selection tests can own feature matrices; the last real native witness cannot be removed. |
| WebView preview, scrolling, recovery and inactive rendering | Browser/native composition and asynchronous paint/navigation are external | A private preview resource owner could make arbitrary `EditorApp.webview = None` teardown impossible and retire corresponding source-pattern guards. Keep composed preview and actual input evidence. |
| Core architecture source guards | `core` still exposes standard-library path APIs; the compiler does not forbid filesystem/thread/environment effects | A reviewed `no_std` + `alloc` core and path adapters could enforce that boundary at compile time. Until then the lexical guard catches a real dependency escape. |
| Settings, tabs, search, explorer, activity and command UI | Pure/semantic tests own transitions; native journeys detect focus/routing/wiring | Prefer one native input witness per shared routing mechanism plus deterministic control tests. No full journey was deleted merely because a state enum exists. |
| Workspace/Overleaf/Git/GitHub persistence and authentication | Real files, separate owners, cookies, helper processes and recovery still interact | Single-owner transaction/session adapters can move more merge, stale-reply and failure combinations into deterministic tests. Retain local-server and native session/login boundaries. |
| Packaging, migrations, dependencies and startup admission | Cargo/bundle/tool versions and executable behavior are external contracts | Generate one canonical resource/config schema where possible. Schema validation can replace duplicate string checks; executable/bundle admission still needs integration coverage. |
| Optional models, real tools and stress/visual/performance runs | Availability, model execution, renderer behavior and scaling are separate evidence | Keep these opt-in and bounded. Small protocol fixtures can cover error matrices; deterministic tests do not establish output quality, native geometry or cross-platform speed. |

Ignored real-tool, model, native-window and measurement tests remain explicit
opt-in workloads. An unavailable desktop or missing input permission is never a
pass. Screenshots remain reserved for visual relationships; this harness work
does not establish visual composition.

## Native journey inventory

The inventory below records every registered leaf at audit time. All are retained;
the dialog protocol and specified comment/math matrices are consolidated as
described above. `all` and `tex` are selectors, not additional journeys. Treat
the source registry as authoritative when adding new coverage.

<!-- Journey inventory generated from the executable registry at audit time. -->

| Source under `scripts/` | Native journeys retained |
| --- | --- |
| `desktop_ui_autofix_journeys.py` | `tex_autofix`, `tex_math_conversion` |
| `desktop_ui_tex_refactor_journeys.py` | `tex_refactor` |
| `desktop_ui_explorer_journeys.py` | `explorer_files` |
| `desktop_ui_overleaf_journeys.py` | `overleaf` |
| `desktop_ui_overleaf_session_journeys.py` | `overleaf_session` |
| `desktop_ui_git_journeys.py` | `git_setup` |
| `desktop_ui_github_journeys.py` | `git_github` |
| `desktop_ui_activity_journeys.py` | `activity` |
| `desktop_ui_indentation_journeys.py` | `indentation` |
| `desktop_ui_tool_defaults_journeys.py` | `tool_defaults` |
| `desktop_ui_settings_links_journeys.py` | `settings_links` |
| `desktop_ui_ltex_journeys.py` | `ltex` |
| `desktop_ui_liquid_journeys.py` | `liquid_selection`, `liquid_streaming` |
| `desktop_ui_latexdiff_journeys.py` | `latexdiff` |
| `desktop_ui_texdoc_journeys.py` | `tex_documentation` |
| `desktop_ui_citation_journeys.py` | `citation_import` |
| `desktop_ui_asset_layout_journeys.py` | `pdf_layout` |
| `desktop_ui_inactive_preview_journeys.py` | `inactive_preview` |
| `desktop_ui_version_journeys.py` | `file_versions` |
| `desktop_ui_palette_journeys.py` | `command_palette` |
| `desktop_ui_jump_journeys.py` | `preview_jump`, `tex_preview_scroll` |
| `desktop_ui_tex_journeys.py` | `tex_editing`, `tex_intelligence`, `tex_save`, `harper_math_prose`, `harper_settings` |
| `desktop_ui_tex_editor_features.py` | `tex_backslash_closers` |
| `desktop_ui_tex_magic_journeys.py` | `tex_magic` |
| `desktop_ui_image_journeys.py` | `images` |
| `desktop_ui_newest_journeys.py` | `problems_focus`, `preview_find`, `tables`, `arxiv_tables` |
| `desktop_ui_math_grid_journeys.py` | `math_grids` |
| `desktop_ui_journeys.py` | `drawing`, `rectangle`, `pdf_output`, `editing`, `search`, `tabs`, `tab_drag`, `dialogs`, `startup_preview`, `diagnostics`, `completion`, `layout`, `explorer_toggle`, `explorer_scroll`, `folding`, `selection_wrap`, `git_hunks`, `preview_keyboard`, `pdfium_resize`, `closing`, `controls`, `settings_search`, `editor_preferences`, `typing_particles`, `background`, `background_fades`, `context_menu` |
| `desktop_ui_snippet_journeys.py` | `snippets` |
| `desktop_ui_ime_journeys.py` | `ime` |
| `test-desktop-ui.py` | `panels`, `terminal_find`, `terminal_load`, `find`, `empty`, `focus`, `preview` |
| `desktop_ui_terminal_theme.py` | `terminal_theme` |
| `desktop_ui_workspace_switch_journeys.py` | `workspace_switch` |
| `desktop_ui_partial_journeys.py` | `partial_preview`, `partial_zoom_stress`, `partial_cache_edges`, `partial_dividers` |
| `desktop_ui_comment_controls_journeys.py` | `comment_controls`, `comment_controls_settings` |
| `desktop_ui_effects_journeys.py` | `effects_pacing` |
| `desktop_ui_diagnostic_fix_journeys.py` | `diagnostic_fix`, `diagnostic_fix_setup` |

Snapshot: 84 leaf journeys; 60 in the shared default session.
