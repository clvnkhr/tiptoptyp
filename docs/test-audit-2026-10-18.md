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

## Coverage deliberately retained

- Geometry, parser, session, save, stale-response, worker and allocation tests
  catch edge cases that a happy-path desktop journey cannot establish.
- The 223 preview JavaScript assertions exercise production navigation,
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
| Python detector contracts | 156 passed in root rerun. |
| Preview JavaScript contracts | 223 passed in root rerun; browser/partial native self-checks also passed. |
| Required checks | Format passed; strict all-target Clippy passed; complete Rust suite 2,286 passed / 63 ignored across 17 targets; xtask 32 passed / 1 ignored. |
| Profiling feature and relocated probes | Strict Clippy and the complete profiling suite passed (2,294 tests, 63 ignored). Both relocated probes executed successfully from the debug test binary (execution validation, not performance measurements); the unique allocation regression passed in the normal suite. |
| Post-change PDFium ignored regressions | Both worker and view tests passed again against the audited binary. |
| Explicit native-window check | Passed native drag guard, transparency/blur, resize, input, cleanup and sibling isolation contracts on macOS. |
| Desktop-test feature | Strict all-target Clippy passed; all 8 actual Find-bar tests passed with native observation hooks enabled. |
| Real-app Find and partial zoom stress journeys | Blocked: macOS locked before launch; Accessibility/input permissions are present, but `desktop=false`, `locked=true`. The optimized audited app is built and preserved. |
| Native zoom compositor runner | Harness compiled; execution blocked by the locked desktop. Python admission checks are not a native pass. |

Native results must record the actual running build/backend and remain separate
from Rust semantics or viewport screenshots. Non-macOS explicit-native rejection
branches cannot be executed on this Mac. No visual verification or cross-platform
performance guarantee is claimed by this audit.

Build/readiness metadata and raw logs are preserved locally in
`.tiptoptyp/test-audit/2026-10-18/` (ignored generated evidence). The prepared app
is `tiptoptyp Dev 0.1.0 (f375b20964b3-dirty.1792323534)`, optimized with
`desktop-ui-tests`, SHA-256
`ab45bfa12df65d292edec4d6b80ccf3f6cb95f0f6ed56cec21bd139649951ecf`.
Host: macOS 14.6.1 ARM64, 1512×982 logical desktop at 2×. No effective preview
backend was observed in the composed app because the journeys could not launch.

Remaining native commands, after unlocking (run sequentially from the checkout):

```sh
python3 scripts/test-desktop-ui.py --journey find --release --executable .tiptoptyp/test-audit/2026-10-18/tiptoptyp-audit
python3 scripts/test-desktop-ui.py --journey partial_zoom_stress --release --executable .tiptoptyp/test-audit/2026-10-18/tiptoptyp-audit
python3 scripts/test-preview-zoom-native.py --binary target/debug/deps/native_preview_zoom-472ff33c3eb31c03 --mode both --transition-regression --scroll-race --dark
```

The unchanged debug-linker `__eh_frame` size warning also occurred in the
baseline; strict Clippy is clean. Source changes were saved in six focused
commits (`738c1a3`, `2a689c0`, `985ea2d`, `e2c2c30`, `a8719b3`, `b9039a1`).
