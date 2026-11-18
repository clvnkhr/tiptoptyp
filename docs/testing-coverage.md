# End-to-end tests and coverage

The preview regression lane exercises real services without requiring a desktop:

- `src/app/preview_controls/e2e.rs` compiles a three-page Typst document, renders
  it through the real PDFium worker, and operates the shared controls through
  accessibility queries. It covers outline/history, search, query selection,
  zoom/fit, resizing, minimize/reopen, pop-out/return, PDF replacement and recovery
  after invalid PDF bytes. The embedded child-window test checks app ownership
  and state; it does not establish native window composition or OS focus behavior.
- `scripts/test-preview-e2e.cjs` runs the pinned Tinymist frontend in Chromium.
  It clicks an actual internal link, checks Back/Forward, page selection across
  resize, compiled outline destinations, zoom/fit, keyboard isolation, palette
  changes without rerendering, and an unsaved source update over the real control
  connection. It fails on browser errors and closes its own browser/server.
- `scripts/test-preview-navigation.mjs` covers bounded scheduling, page geometry,
  history anchors, accumulated zoom, renderer replacement and idle behavior.

The new tests reproduced a PDF reading-position jump during fit-width resizing
and Tinymist's use of glyph bounds instead of paper bounds for page navigation.
PDFium now restores its page-relative position when fit width or the containing
view changes. Pointer zoom retains its separate pointer anchor. Tinymist measures
the actual paper rectangle and clears an old link anchor for explicit page jumps.
Preview search and history buttons also have meaningful accessibility labels.

## Reproduce

Fetch pinned tools and PDFium with the existing `xtask fetch-sidecars` and
`xtask fetch-pdfium` commands. Then run:

```sh
cargo test --bin tiptoptyp e2e_pdf -- --ignored
node --test scripts/test-preview-navigation.mjs
TIPTOPTYP_PLAYWRIGHT=/path/to/playwright \
TIPTOPTYP_TEST_TINYMIST=/path/to/tinymist \
node scripts/test-preview-e2e.cjs
```

The browser lane uses Playwright 1.63.0 with its installed Chromium, or the
executable supplied by `TIPTOPTYP_TEST_CHROME`. Dependencies are installed outside
the repository and do not become application dependencies. CI runs both lanes;
the separate native-window job remains opt-in and requires a working desktop.

For coverage, install `cargo-llvm-cov` 0.9.1 and the toolchain's
`llvm-tools-preview` component, then run:

```sh
python3 scripts/measure-coverage.py --real-tools --tex-distribution
```

Omit `--tex-distribution` without pdfLaTeX, XeLaTeX, LuaLaTeX and SyncTeX. Omit
`--real-tools` for the regular Rust suite alone. Tool-dependent tests are selected
explicitly; performance probes are not treated as end-to-end tests. Missing
required tools or failing assertions fail the run rather than silently skipping.
The runner clears previous profiles, records a standard-suite baseline, adds the
requested integration runs, and produces a browsable report in
`.tiptoptyp/coverage/latest/html/index.html`, with per-file JSON and run metadata.
CI retains this directory as the `rust-coverage` artifact.

Coverage includes the Rust application and core workspace, including inline unit
test bodies. Standalone test files, vendored code and build scripts are excluded.
JavaScript, native libraries, sidecar executables, OS interactions and the separate
`xtask` crate are outside this percentage. Stable Rust does not provide branch
coverage here. A high line percentage does not prove that all UI event orderings
work. No desktop composition verification is claimed by these headless runs.

These fixes introduce no background jobs or repaint loops. Browser tests still
observe zero renderer reruns while changing the palette, and deterministic tests
check that idle input leaves no queued animation frame. The added state checks
have no material expected performance impact; this is not a cross-platform timing
claim.

## Generated invariants and test cleanup (2026-10-25)

The fast suite includes bounded exhaustive and seeded property/model tests.
They use independent representations rather than copying the implementation:

| Area | Oracle and workload |
| --- | --- |
| LSP incremental updates | A strict UTF-16 receiver applies the serialized wire change to the old source. All 160,000 ordered pairs of 400 short Unicode/line-ending sources must reconstruct the new source; 32 sequences of 32 edits also exercise successive coordinate revisions. Full-sync capability and large changed-span allocation checks remain separate. |
| Core edits | All 259 short Unicode/LF/CRLF documents use scalar-vector splices and independent selection bookmarks. Legal batches must produce exact text and positions; an invalid UTF-16 endpoint rejects the entire batch. |
| Incoming remote edits | Known scalar splices and 4,096 constructed two-sided changes around a unique unchanged line check selection affinity, native document ownership and line-anchor stability. |
| Save journal | All 14,641 four-action two-document schedules and 32 fixed 64-action histories compare editor intent with a confirmed server journal. Optimistic snapshots, wrong-owner/generation receipts, duplicate acknowledgements, reconnects and pending-save ownership cannot silently confirm or erase edits. Conflict/deletion-specific regressions remain separate. |
| Authoritative HTTP fixture | Three seeded histories perform 216 actual file API operations plus 72 rejected writes. A separate tree/source/event model checks identity, contents, deletion and failure atomicity. This validates the native test oracle, not Overleaf Cloud. |

Every bound and seed is checked in. Failures print the source construction,
edit/step or action prefix needed to replay them. These are reproducible
properties, not coverage-guided fuzzing or a proof over unbounded inputs.
They add no runtime dependencies or application work. Run them with:

```sh
cargo test generated_ -- --nocapture
cargo test --bin tiptoptyp save_journal_exhausts_short_two_document_schedules -- --nocapture
python3 -m unittest discover -s scripts -p 'test_*contract.py'
python3 -m unittest discover -s scripts -p 'test_overleaf_*fixture.py'
```

Cleanup removes the test-only cursor diff/transition oracle that duplicated the
production diff, six isolated save tests completely covered by the journal,
and repeated per-feature registry/source-token tests. Concrete races, geometry,
parser/security, worker ownership, real-widget tests and failure-detector
negative cases stay: a happy native journey cannot establish those invariants.
Registry checks now cover every selectable method and aggregate membership in
one place. Fixture-routing tests execute the real CLI up to the native launch
boundary and explicitly require that preparation is not recorded as a pass.

The three Overleaf desktop journeys are now one `overleaf` workflow: browser →
file actions → storage/reconnect. Every phase retains its existing real input
assertions and must restore the original local source, tabs, preview and focus.
The shared process catches lifecycle leaks hidden by isolated launches. The final
persistent-copy phase must retain its offline project metadata while releasing
its transport and browser. Corrupted phase outcomes prove the boundary detector
fails before continuing.
Use `python3 scripts/test-desktop-ui.py --journey overleaf --trace-preview`.
This consolidation does not replace native keyboard, WebView or OS-focus
coverage with mocks. See [desktop journeys](desktop-ui-journeys.md) for evidence
and prerequisites. No new coverage percentage is inferred from generated case
counts or fewer test functions.

## Measured on 2026-09-25

On macOS arm64 with Rust 1.98.1 and cargo-llvm-cov 0.9.1:

| Rust scope | Standard suite | With real-tool tests |
| --- | ---: | ---: |
| Entire workspace | 78.19% | 80.89% |
| Application | 77.74% | 80.50% |
| Core | 98.68% | 98.68% |
| PDF viewer | 29.40% | 79.02% |
| Shared preview controls | 62.45% | 84.93% |
| TeX build adapter | 54.70% | 91.35% |
| SyncTeX queries | 39.67% | 89.26% |

These are line percentages on the same instrumented build, with inline unit tests
included as described above. The final run covered 59,221 of 73,210 lines and
5,574 of 6,748 functions (82.60%). The extra run includes fourteen real-tool tests,
including both new PDF UI workflows. The eight JavaScript unit tests and seven
browser scenarios passed separately and do not contribute to the Rust percentage.
The required formatting, strict Clippy, full Rust suite (1,283 tests) and separate
xtask suite (15 tests) also passed.

The largest remaining gaps include the package browser (0%) and the native-view
adapter (7.60%). OS focus, window composition and other unexercised paths still
need the dedicated desktop lane; these results do not make a completeness claim.
[Machine-readable results](coverage/2026-09-25.json) retain the source digest,
commands, tool versions, counts and browser evidence. Full per-file HTML remains
in the local report directory; CI produces its own artifact for each run.
