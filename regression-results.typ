#set document(title: "Developer-owned regression checks — 18 September 2026")
#set page(paper: "a4", margin: 19mm)
#set text(size: 10pt)
#set heading(numbering: "1.")
#set par(leading: 0.55em)

= Regression checks: work taken off your hands

== Latest checklist follow-up

The user passed checks 1–3. Checks 4, 7, 8 and 9 were skipped by the user and are
now developer-owned, with the following automated checks passing:

- *4 — save/cancel:* `save_completion_follows_a_parked_tab_id_after_reorder_not_the_active_slot`
  writes and reads actual disposable files while the save is held behind a
  controlled barrier; `closing_checks_dirty_background_tabs_without_discarding_on_cancel`
  preserves the buffer and cancels the close workflow.
- *7 — background autosave:* `parked_autosave_checks_disk_and_never_overwrites_external_edits`
  writes the inactive tab, verifies its bytes, then verifies a conflicting
  external edit is not overwritten. The save-ownership test above also verifies
  the active sibling is unchanged after reordering.
- *8 — external conflict:* `durable_save_releases_one_close_and_conflicts_require_a_fresh_confirmation`
  exercises actual disk changes and the overwrite-confirmation workflow,
  including a second external edit after the first confirmation.
- *9 — stale results:* `completion_transaction_rejects_changed_revision_epoch_and_owner`,
  `stale_events_are_not_delivered_or_repainted`, and
  `assets_keep_typst_output_and_reject_late_results_after_closing` deliberately
  test obsolete identities/results. This is deterministic admission/routing
  coverage, not a claim that a race was won by clicking quickly in a native UI.

Settings now has one root-owned retained viewport. Secondary toolbar requests
are drained by the shell; native Settings commands route directly to that owner.
Repeated requests raise the same window instead of toggling a document-local
window. Toolbar buttons are unselected action buttons. Preferences continue
through the existing app-wide broadcast path. Unit tests cover two secondary
owners, one child ID and real shell command routing. Native tests opened Settings
from a secondary document, requested it again while open, closed it and reopened
it from the primary toolbar. The short-lived flash itself was not isolated;
todo 234 remains open rather than claiming screenshots disprove it.

Open pickers accept folders as well as files. Folder selection queues a new
workspace window and leaves the current document alone. Explicit folder launch
ignores remembered file history and creates zero tabs. The constructor regression
also verifies no preview server starts until a document is created. A separate
native folder launch displayed the expected Explorer and “No open tabs” view.

Cmd+N no longer calls the preview/service restart path for a non-preview tab.
The new tab gets a private LSP backing without replacing the pinned preview.
The regression asserts unchanged server generation, preview identity, ready
status and compile deadline. In the native run, the preview retained both its
WebView URL and displayed fixture after Cmd+N.

Native verification used an isolated *debug profiling-feature* build, identified
as `d3ca9886aa79-dirty.1789749031`, not an optimized performance comparison.
Fresh app framebuffer captures for Settings and the editor were inspected under
the evidence workspace's `.tiptoptyp/screenshots/`; the editor capture intentionally
omits the WebView and can request a raster compile for capture. It is not evidence
of an ordinary Cmd+N compile. Native whole-window observation independently showed
the original preview. Preview bounds traces are retained in
`followup-interactive/app.log`. The run was deliberately quit before its profiling
deadline, so its incomplete measurement is not used as a performance result.
No before/after timing comparison is claimed after the earlier build-cache clean.
The relevant resource invariants are one Settings surface and no extra preview
restart/job for New. No continuous repaint or runtime logging was added.

Full-suite execution and focused routing evidence are in `followup-full-tests.log`
and `followup-routing.log` under the original evidence directory. These checks
take the skipped work off the user; they do not require expanding the manual list.
Final checks passed: 1,057 test executions, 13 xtask tests, formatting and strict
Clippy. The native empty-folder framebuffer was freshly captured and inspected
at `.tiptoptyp/screenshots/1789749342893-0001-main.png`. The disposable app bundle
was moved to Trash after both native runs; dependency/build caches were retained.

== Earlier execution record

*Follow-up — item 228 fixed:* the original execution record below describes the
pre-fix build. The retained host now reports “No document” rather than panicking
on its empty Typst placeholder. A new regression failed at the exact original
panic before the fix, then passed through hidden/visible Settings and document
resumption. Menu/dispatch tests reject plain Open without a document window and
permit New Window, Open in New Window and Settings. An isolated native app stayed
alive after closing documents, reopened documents and displayed the Open in New
Window picker. Completing file selection was not verified: automation clipboard
delivery in the picker timed out. The no-window runner additionally exposed a
CGL panic because it retired the root before the normal UI pass; it now follows
the native end-of-frame retirement/retained-surface registration sequence. The
debug and optimized native no-window runs both complete successfully. The final
optimized run used 5 seconds warmup plus 5 seconds measurement, with no recorded
UI passes during that idle interval. It is recorded in
`lifecycle-no-window-final.log`; all follow-up logs share the original evidence
directory. Formatting, strict Clippy with and without profiling, 1,053 default
test executions, 1,059 profiling-feature executions and 13 xtask tests pass.
The temporary isolated app bundle was moved to Trash after testing; the user's
app was not stopped. Item 229 remains open.

*This is an execution record, not another manual checklist.* I recovered the
original 107 cases from the earlier checklist PDF and extracted the portions
that can be tested without your involvement. The ten-check user checklist is
not expanded. A passing unit test is not a passing native interaction; many
original cases combine both, so there is deliberately no “107 checks passed”
claim.

== Result and priority findings

- *1,052 default-suite test executions passed*, with 16 opt-in tests ignored.
  The profiling-feature suite passed 1,058 executions. These counts overlap
  substantially and include shared code exercised in multiple test binaries;
  they are not distinct end-to-end scenarios.
- *All 16 opt-in tests passed* in an optimized profiling build, including real
  Tinymist, compiler recovery, real inline/block miTeX, and resource probes.
  The actual AppKit drag-guard harness passed separately. All 13 xtask tests,
  formatting and strict Clippy (ordinary and profiling feature) passed.
- *Native failure found:* the isolated interactive app panicked at
  `src/app/settings_view.rs:98`, entering the supposedly unreachable Typst
  branch while preparing Settings status. Its log is preserved. The function
  assumes no preview means the document cannot be Typst, but
  `typst_preview_available()` also returns false when the tab store is empty.
  The independent `no-window` profiling scenario reproduced the same panic,
  exiting with status 101. Its backtrace goes through `hidden_host_ui` into
  `show_settings_window`: the retained host is preparing Settings with no tabs.
  The exact timing of the first interactive occurrence is not isolated, but the
  second reproduction requires no manual input. Do not consider window lifecycle
  accepted merely because its state-model tests pass.
- *Keyboard acceptance remains partial:* after a settled native preview click,
  Cmd+Left followed by Option+Right and typing edited the expected word.
  An immediate click/key sequence did not insert its test character. This
  could be asynchronous jump delivery and is not proof of the reported
  intermittent problem being resolved.

No application source was changed during this test pass. Findings below are
not presented as fixed.

== Build and safety record

Tested commit `d3ca9886aa79fc4eee4235b9c5867bb0d3b85d27` on macOS arm64
14.6.1, Rust 1.96. The working tree was clean before this report.
Optimized executable: `target/profiling/tiptoptyp`, built with profiling enabled
and `RUSTFLAGS=-Cforce-frame-pointers=yes`. SHA-256:

```text
658072784eb89b66901b1aa22d65c794649884429f9aecf049687dfa8835c69a
```

The native interactive run used a uniquely named temporary app bundle containing
that exact executable, disposable documents, a bounded 300-second profiling
session and isolated preferences. Your existing app and its Tinymist processes
were not terminated. Tests involving Git used the suite's disposable
repositories, not your working index or remote.

Tool integrations used repository-pinned Tinymist v0.15.2 and Typst v0.15.1;
Poppler was 26.04.0. Raw logs and interactive fixtures are retained locally in
`.tiptoptyp/regression-20260918.ju10Eb/` (Git-ignored).

== Extracted developer checks

The IDs below refer to the original 107-case plan. “Covered portions” means
the named assertions ran successfully, not that every sentence in the original
case has been validated. Native gaps are collected separately instead of
turning them into more work for you.

#let area(title, ids, body) = block(above: 0.7em)[
  *#title* — #ids #linebreak() #body
]

#area("Startup, windows and menus", "B01–B05, W01–W05, A06–A07", [
  Build identity verified by hash. Automated retained-root lifecycle, command
  ownership, dormant Settings, close approval and secondary repaint tests passed.
  Native two-window creation, separate editing, dirty-close Cancel and Discard
  were exercised. The subsequent panic prevents an overall lifecycle pass.
])
#area("Editing and focus", "E01–E04, E06–E07, V03", [
  Editor state, Unicode offset conversion, undo/selection and source-jump tests
  passed. Native direct-editor word/line navigation, saving, isolated undo and
  settled preview-to-source navigation worked. Immediate focus handoff remains
  inconclusive; IME composition was not tested.
])
#area("Hover and popup policy", "H01–H09, R02", [
  Ran delay ownership, syntax-boundary targeting, safe-triangle geometry,
  transient pointer gaps, child ownership, move-away/scroll dismissal, expansion,
  link dispatch and cache/culling tests. Real Tinymist accepted the supplied
  epsilon/sigma example's hover positions. These do not simulate the actual
  mouse crossing between native windows. The available interaction tool has no
  pointer-move-only operation, so diagonal handoff was not falsely checked off.
])
#area("Tabs, Explorer and ownership", "T01–T06, X01–X05, X07", [
  Tab identity, reorder/remapping, preview pinning, image/PDF pane policy, close
  reassignment, stale results and empty-workspace tests passed. Explorer minimum
  width, closing-frame preservation, reopen width, tree/index invalidation and
  warning-visibility tests passed. Native dragging reordered two tabs without
  moving the window; switching to the second document retained the first's
  preview. A Cmd+1 hide/reopen pair ended at the original width, but constrained
  resizing and persisted restart sizing were not established by that sample.
])
#area("Saving and external changes", "S01–S10", [
  Ran canonical-byte writes, atomic/durable receipts, parked-tab save ownership,
  Save As rebinding, autosave conflict protection, in-flight edits, failure paths,
  same-path write serialization and line-ending tests. Native disposable-file
  Save changed the intended disk contents; Cancel retained a dirty second
  window. These do not constitute a native two-editor conflict-dialog test.
])
#area("Completion and delimiter insertion", "C01–C07", [
  Completion matching/acceptance, stale results, TeX symbols, Unicode/fonts and
  real TextEdit event tests passed. Fence tests include typed and batched events,
  middle-of-document insertion and insertion before existing content. Math
  expansion, two-space indentation, pair skipping and undo are deterministic
  tests, not just inspection of helper return values.
])
#area("Find and folding", "F01–F05", [
  Search modes, zero-width regex progress, replace/history, fold projection,
  reveal rules and marker geometry passed. The optimized large-document folding
  probe also passed. No prolonged native scrolling/selection trial was performed.
])
#area("Compile, recovery and diagnostics", "V01–V09", [
  Real watcher compilation, error recovery, Tinymist preview cycles and private
  unsaved-document formatting passed. Recovery budgets, stale session rejection,
  capability reporting, status and diagnostic ownership passed deterministic
  tests. Native live preview updated disposable edits. Five deliberate native
  server crashes, export and composed-view clipping were not tested.
])
#area("Assets and memory policy", "P01–P03, P05, R04", [
  Asset failure/stale-result handling, page selection and 1/20/100-page bounded
  residency tests passed, including the optimized residency probe. These test
  accounting and admission policy; they do not measure actual GPU-driver
  allocations or a native 100-page scrolling workload.
])
#area("miTeX", "M01–M06", [
  Eligibility, inline/block projection, canonical saving, unfinished-input refusal,
  positions, round trips and disabled-mode no-projection tests passed. Real
  inline/block output compiled. Disabled-mode and projection-cache probes ran;
  the ordinary editor was not made to construct projection state by the tests.
])
#area("Git", "G01–G06 and local portions of G07", [
  Compact action-row interaction, navigation, partial stage/unstage, undoable
  revert, changed-HEAD and concurrent-index preconditions passed. Disposable Git
  tests cover quoted paths, unborn repositories, executable mode, conflicts,
  CRLF and absent final newline. No production remote operation was attempted.
])
#area("Settings, themes and responsiveness", "A01–A05, A07, R01, R03, R05–R06", [
  Narrow-row grouping, chip wrapping, hint sizing, theme controls, font selection,
  shortcut routing, preference coalescing, catalog reuse and independent deferred
  painting tests passed. Worker admission, bounded indexing, stale-result
  rejection and no-extra-repaint invariants also passed. Native Settings status
  still has the panic described above; those tests do not certify all lifecycle
  combinations.
])

== Concrete native samples

Using disposable main.typ and other.typ, I observed:
- Main-file live preview, disk save and undo.
- Second tab displaying different source while the first remained preview owner.
- Tab drag changing order, with the window staying in place.
- A second native window rendering “SECOND WINDOW ONLY” independently.
- Cancelling its dirty close retaining the unsaved text; subsequently discarding
  only that disposable buffer returning to the first window with its source and
  pinned preview intact.

These are single samples, not the ten repeats requested by the original plan
for intermittent failures. A subsequently created external file was not observed
in the surviving Explorer before the test process ended, so watcher survival is
not claimed as a native pass.

== Performance and visual evidence

The hover scenario failed to finish its initial popup capture within 120 seconds;
the runner terminated its own test process. No valid hover timing was produced.
Its app log was empty, so this establishes a failed test setup/run, not a proven
application deadlock. The no-window scenario failed with the panic above.

Six other scenarios completed serially with the same optimized executable,
Catppuccin Latte QA fixtures, 5 seconds of warmup and 5 seconds of measurement,
without concurrent builds or other test runs: main, Settings, four windows, Find,
font picker and large source. Each summary was complete with no dropped scopes.
There were no recorded instrumented UI scopes in these idle intervals; that
means no observed instrumented passes, not zero rendering cost. The supplementary
process CPU-time readings increased by approximately 0.01, 0.01, 0.03, 0.02,
0.01 and 0.02 seconds respectively. These coarse process readings bracket the
runner's interval and exclude sidecar processes. No CPU stack sampler was used.
Your separate app remained open; this is a short local idle characterization,
not a latency, GPU-memory, active-scroll or cross-platform guarantee.

Exact run directories, binary hashes, fixture hashes and logs are identified by
the `profile-*.log` files in the evidence directory. The failure follow-ups are
todo items 228–229.
No before/after performance improvement is claimed: this task changes no runtime
code and has no pre-refactor comparison executable. The first optimized probe
run overlapped other work; its functional assertions count, its timings are not
used as controlled benchmark results.

I then repeated all 16 opt-in tests serially after the GUI scenarios and other
tests stopped; all passed again (`optimized-isolated-tests.log`). A separate
optimized headless popup probe also passed (`tooltip-cost.log`). Its 17,470-byte
card averaged 0.068 ms warm with culling versus 1.850 ms without; the cold culled
render took 7.199 ms. This supports the rendering policy, but does not clear
native pointer delivery, WebView focus or scrolling latency. PDF residency
accounting admitted three pages for both the 20- and 100-page synthetic cases,
with 24 MiB decoded plus 24 MiB texture accounting; these are policy counters,
not measured GPU allocations. Disabled-miTeX checks performed no projection
work, and the optimized core/adapter loops were of similar magnitude across
five rounds. These are local probes, not statistical performance guarantees.

Fresh main and Settings viewport PNGs were captured by the app and inspected.
The ordinary split view had a usable Explorer, aligned tab controls, visible
source and rendered preview; the Settings capture had intact control groups
within its viewport. This does not prove narrow-window behavior, hover handoff,
native child composition or window shadow correctness. Native whole-window
observations above are separate evidence, not framebuffer claims.

== Remaining gaps — developer work, not a new assignment for you

Prioritize fixing the reproduced Settings/lifecycle panic, then automate
the immediate WebView-to-editor key handoff and native popup crossing. A popup
test that bypasses native event delivery is insufficient for this regression.

Still unverified: external-display/scale changes (B06); IME (E05); native
filesystem dialogs and destructive Explorer operations (X06); preview export
(V10); external PDF links (P04); package operations (A08); remote Git actions;
full accessibility/contrast judgments; prolonged CPU/GPU/RAM observation and
30-minute soak (R07). Preference restoration is isolated by construction (Z01);
honest acceptance requires leaving these gaps open (Z02).

== Reproduction commands

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
cargo test --manifest-path xtask/Cargo.toml
cargo clippy --all-targets --features profiling -- -D warnings
cargo test --features profiling --no-fail-fast
TIPTOPTYP_NATIVE_DRAG_TEST=1 cargo test --test native_window_drag
RUST_BACKTRACE=1 cargo xtask profile --scenario no-window \
  --warmup 5 --seconds 5 --sampler none --skip-build
```

For the opt-in suite, set `TIPTOPTYP_TEST_TINYMIST` and
`TIPTOPTYP_TEST_TYPST` to the absolute pinned sidecar paths, then run:

```sh
RUSTFLAGS=-Cforce-frame-pointers=yes cargo test \
  --profile profiling --features profiling --no-fail-fast \
  -- --ignored --nocapture --test-threads=1
```

Individual log files retain exact test names and results. Do not add together
ordinary, profiling and repeated opt-in counts as independent coverage.
