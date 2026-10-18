# Test and legacy-code audit — 2026-10-18

Scope: main checkout `/Users/calvinkhor/Documents/GitHub/mytypst`, starting at
`f375b20`. Inventory covered Rust source/tests, feature/platform-gated native
harnesses, preview JavaScript tests, Python detector contracts and CI selectors.
Ignoring a test, having only test callers, or overlapping a desktop journey was
not by itself evidence that coverage could be removed.

## Removal evidence and retained coverage

The pre-change inventory used `git grep -n` against `HEAD` across `core`, `src`
and `tests`; current-tree searches confirmed the replacement call paths.

| Removed or relocated | Evidence of obsolescence | Coverage retained or improved |
| --- | --- | --- |
| `core/src/workflow.rs::take_continuation` and its test-only `src/workflow.rs` adapter | No runtime caller; callers were tests and that adapter. The method released a continuation without the receipt/token checks used by the `complete_save` path in `src/app/saves.rs`. | `save_before_close_has_exclusive_phases_and_one_continuation` now uses the real completion path and verifies exactly-once release. Save/close contracts retain cancellation, wrong-owner, new-edit and stale-receipt cases; write failure now exercises `complete_save(Err(...))`. |
| Core preview `pages_mut`, `accept_raster`, `fail_raster`, `error`, and obsolete `RasterState` branches | `pages_mut` had no callers. Raster completion/failure APIs had only core tests and one application test fixture. Runtime PDF pixels/errors belong to `PdfiumPreviewRuntime`; static assets use `replace_asset`, PDF provenance uses `accept_artifact`. | Core tests still require stale pixels/provenance retention, PDF replacement/invalidation, image replacement and clearing. Actual PDFium worker/view tests retain stale-result rejection and error recovery; both were executed successfully before removal (see below). |
| `src/app.rs::find_step_for_enter`, `FindStep`, and the detached test | Both were `cfg(test)` only; the UI never called this helper. It could pass while the actual Find field failed. | `find_bar::tests::enter_navigates_actual_find_field_only_and_retains_focus` presses Enter/Shift+Enter on the real widget, checks one intent per press, retained focus/query and no navigation when unfocused. |
| `ThemeTransform::{is_identity,apply_color32}` and conversion-only test | These methods were `cfg(test)` only and used exclusively by their own tests. Runtime theme conversion uses `apply_rgba` and palette/theme transforms. | NaN normalization now asserts the actual `apply_rgba` result. `identity_and_full_turns_are_bit_exact`, `inversion_is_exact_self_inverse_and_preserves_alpha`, and semantic/theme mapping tests remain. |
| `mitex_document::Document::take_history_reset` | Only two assertions called it. Editor paths already inspect the reset flag and explicitly call `set_history_reset(false)`. | `toggles_preserve_disk_identity_dirty_work_and_exact_spelling` now follows the actual inspect/reset sequence, retaining epoch, undo, save identity and exact-spelling checks. |
| Integration wrappers `tests/{architecture_cost,completion_payload_cost}.rs` | `#[path]` imports recompiled 4 transport, 18 project-index and 2 protocol unit tests: 24 duplicate executions. The payload wrapper also duplicated the allocation observer. | Unique allocation regression and both ignored performance probes move to `src/performance/`, importing the actual crate modules and shared `test_allocations`. Documented probe commands now target `--bin tiptoptyp`. No probe or unique assertion was discarded. |
| Outer `Path` import in `desktop_ui_terminal_theme.py` | No Python-level use; the embedded probe source has its own independent import. | Probe source and terminal theme journey are unchanged. |

## Genuine defects fixed in the test infrastructure

- Architecture checks used `split("#[cfg(test)]").next()`: an inline test counter
  could hide all subsequent production functions. Syntax-based masking now
  removes only actual `cfg(test)` items/fields/blocks, preserving following code,
  nested modules, raw strings and optional platform/feature implementations.
  Two adversarial scanner tests protect this. `syn`/`proc_macro2` additions are
  development dependencies, already present transitively; no runtime dependency
  was added.
- Partial-preview assertions accepted missing counters/page flags, duplicate or
  invalid indices, and boolean/negative counts. Four new contract tests cover
  21 invalid-evidence cases reproduced against the old assertions.
- Zoom burst checks skipped coalescing assertions when the `deferred` field was
  missing. They now require the current schema, all 20 screenshot-cache hits,
  one final vector request and no unfinished handoff.
- The native zoom runner always selected the ARM sidecar, ignored
  `TIPTOPTYP_TEST_TINYMIST`, and could attempt native launch on Linux. Six pure
  contracts cover ARM/Intel selection, override/home expansion, missing or
  non-executable helpers and unsupported hosts. Old-code reproductions failed.
  Explicitly requested Rust native zoom/drag checks now fail off macOS rather
  than silently returning success; ordinary opt-out behavior remains explicit.
- Fast CI now discovers every `test_*contract.py`, including previously orphaned
  reveal, Git and package detector tests. The desktop workflow exposes the full
  current journey registry. Two contracts prevent selector/dropdown drift;
  they failed against the old workflow. The meaning of the combined `all`
  journey is unchanged, including intentional standalone/stress exclusions.

## Defects exposed by the native reruns

- The cached edge-wheel journey could sample `capturing=true` and reject a
  healthy replacement token as if it were an interaction failure. It now waits
  for 250 ms of quiescent capture state before input; the token-preservation
  assertion remains. Thirteen invalid-precondition cases protect that admission.
- The post-wheel check could falsely pass an active/pending gesture or an
  unready/non-top renderer if its cache token survived. Eighteen negative cases
  now fail; fresh ready/top geometry and idle gesture state are required. A
  background capture is allowed because a successful full-mode receipt shows
  that valid state; retained-pixel/token equality remains mandatory.
- A later real-app rerun (`run-0hd6r980`) exposed another baseline race:
  native ownership was idle while the observed JavaScript snapshot still had
  revision 25 and its two-frame handoff pending. The prior handoff then replaced
  token 4 with token 6 after the wheel. Baseline admission now requires both
  owners to be idle; 33 adversarial subcases reproduced the old false pass.
- The divider anchor oracle rejected legitimate 0.429 px leading-page padding.
  It now models signed offsets at the actual first/last document edges, keeps
  the 1.1 px anchor tolerance and still rejects missing page geometry. It does
  not substitute a center anchor or discard the alignment check.
- Native Rust and Swift fixtures still called the palette API with four/two
  arguments. Three JavaScript `TypeError`s were real fixture failures. Calls
  now supply all five current arguments; Node tests execute the actual extracted
  calls. The Swift positive sequence passes all four palette/reload stages;
  omitting load replay deliberately fails after three valid stages.

## Coverage deliberately retained

- Geometry, parser, session, save, stale-response, worker and allocation tests
  catch edge cases that a happy-path desktop journey cannot establish.
- The 226 preview JavaScript assertions exercise production navigation,
  snapshot buffering, bounded work, cache reuse and request ownership.
- Native compositor/worker fixtures, palette negative cases, partial-render
  phase sampling and pinned-frontend browser protocol tests remain distinct
  checks. Their opt-in status is not a reason to remove them.
- Test fixtures that call current production helpers remain; unlike the removed
  Find helper, they do not implement a disconnected replacement algorithm.

## Validation and limitations

Results below distinguish deterministic checks from checks requiring an unlocked desktop.
The starting suite passed **2,309 tests**, with **63 ignored**, across 19 targets.
There is no application speed claim: moving probes removes duplicate test work,
not a measured runtime bottleneck.

| Check | Result |
| --- | --- |
| Baseline ignored PDFium worker | Passed `pdfium::tests::native_worker_reuses_documents_rejects_stale_and_survives_errors`. |
| Baseline ignored PDFium view | Passed `app::pdfium_view::tests::native_view_retains_pixels_during_updates_and_errors`. |
| Architecture boundary tests | 25 passed in focused run. |
| Python detector contracts | 168 passed after the native followup fixes, including invalid capture, stale renderer and unfinished handoff cases. |
| Preview JavaScript contracts | 226 passed after the native followup fixes; browser/partial native self-checks passed earlier. |
| Final required checks | Format and strict all-target Clippy passed; complete Rust suite 2,286 passed / 63 ignored across 17 targets; xtask 32 passed / 1 ignored. Final desktop-test-feature strict Clippy also passed. |
| Profiling feature and relocated probes | Strict Clippy and the complete profiling suite passed (2,294 tests, 63 ignored). Both relocated probes executed successfully from the debug test binary (execution validation, not performance measurements); the unique allocation regression passed in the normal suite. |
| Post-change PDFium ignored regressions | Both worker and view tests passed again against the audited binary. |
| Explicit native-window check | Passed native drag guard, transparency/blur, resize, input, cleanup and sibling isolation contracts on macOS. |
| Desktop-test feature | Strict all-target Clippy passed; all 8 actual Find-bar tests passed with native observation hooks enabled. |
| Real-app Find | Passed `run-3s7exjn7` through native input, with the actual running build/hash verified. |
| Real-app partial zoom stress | Final stricter run `run-j34fhbt1` passed both observed Tinymist modes: 20 inputs → 20 cache hits/one vector request/one handoff each, six divider drags per mode, focus release, edge wheels and 27 coverage checks. Earlier failures remain retained. |
| Native partial compositor | Final clean-cadence run `native-partial-cadence` passed all 18 transitions, blocked WebContent, scroll race and dark palette. |
| Native full compositor | Final clean-cadence run `native-full-cadence` still fails the unchanged eight-second mid-document idle-image check. Four native keys remain responsive during the block and yield one final vector request. Dense full-mode capture readiness/latency remains unresolved; this is not a passed transition run. |
| Native Swift palette fixture | Four positive color/reload stages passed. The missing-reload-replay negative failed as intended after three valid stages. |

Native results are separate from deterministic Rust semantics. The earlier
locked-desktop blocker is resolved. The real-app runs above used the same
preserved optimized `desktop-ui-tests` app:
`tiptoptyp Dev 0.1.0 (f375b20964b3-dirty.1792323534)`, SHA-256
`ab45bfa12df65d292edec4d6b80ccf3f6cb95f0f6ed56cec21bd139649951ecf`.
Host: macOS 14.6.1 ARM64, 1512×982 logical desktop at 2×.

Build receipts, original failures (including all three palette reproductions)
and followup logs remain in `.tiptoptyp/test-audit/2026-10-18/`. Final per-mode
receipts/commands are in `native-{partial,full}-cadence/`; both use native probe
SHA-256 `8d3f2217a596707cdb4e8058d9db350d755a5769243fc72f428e865b5b87a51f`.
Earlier `native-compositor-fixed/` failures are retained. No speculative production preview fix,
assertion removal or timeout increase was made. The clean probe observes at
50 ms cadence and retains its eight-second idle-image timeout.

Temporary tracing observed the existing render-completion notification and a
capture retry; explicit settle was not causal in the traced recovery. An
indefinite stall or lost wakeup is not established. Diagnostic instrumentation
was removed. The native fixture is 640×420 with a three-second warmup and actual
pinned Tinymist; it does not establish physical trackpad or GPU timing.

Four fresh palette PNGs, three earlier native compositor PNGs and three fresh
final compositor PNGs were inspected.
These establish fixture pixels only, not composed editor-window geometry.
Non-macOS explicit-native rejection branches and Intel native execution cannot
be exercised on this ARM Mac. No cross-platform performance guarantee is claimed.

The unchanged debug-linker `__eh_frame` size warning also occurred in the
baseline. Initial cleanup commits: `738c1a3`, `2a689c0`, `985ea2d`, `e2c2c30`,
`a8719b3`, `b9039a1`; native followups: `6dfb28a`, `b919720`, `d2afb70`,
`bd82575`. Final check statuses are also retained in
`.tiptoptyp/test-audit/2026-10-18/final-verification.json`.
