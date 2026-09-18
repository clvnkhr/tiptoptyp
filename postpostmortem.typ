#set document(title: "tiptoptyp: post-postmortem", author: "Engineering review")
#set page(paper: "a4", margin: (x: 20mm, y: 19mm), numbering: "1")
#set text(size: 10.5pt)
#set par(leading: 0.6em)
#set heading(numbering: "1.1")
#show raw.where(block: true): set text(size: 8.5pt)

= Post-postmortem: we reorganized more than we simplified
18 September 2026 · Source audit, including the current uncommitted worktree

== Direct answer

*Yes, `app.rs` is still huge: 12,341 physical lines.* It is 1,639 lines smaller
than the first postmortem's snapshot, a reduction of 11.7%. That is a real
readability improvement, but not a small application coordinator. The root plus
its child modules is now *34,890 lines*, up 653 over that same snapshot.

Across tracked Rust source, we went from *93,942 to 95,047 lines*: *+1,105*.
Using the corrected test-counting approximation below, that is about *802 test
lines and 303 other lines*. The latest cleanup itself did remove 74 production
lines; its 89 regression-test lines made its total grow by 15. That small saving
does not offset the broader campaign's growth.

*The mistake was treating an extraction plan as a deletion plan.* We primarily
changed where code lives and how dependencies are expressed. We also repaired
real bugs, which needed checks and tests. We did not identify enough redundant
implementation to delete before starting. The outcome should not have been
expected to meet a substantial total-LOC reduction goal.

The recommendation is not another five broad extractions. First do small,
deletion-first consolidations with explicit accounting. Separately decide
whether making the root file easier to navigate is worth a near-neutral move.
Do not sell the second activity as the first.

= What the numbers actually say

== Comparable snapshots

Counts include comments, blanks, tests, build support and xtask, but exclude
dependencies, generated artifacts and non-Rust documents. Each tracked Rust
file is counted once. WORKTREE means current file contents over HEAD `74cf297`,
including the pending preview work and cleanup. The historical report is a
snapshot, not a live counter.

#table(
  columns: (1.9fr, 1fr, 1fr, 1fr), inset: 5pt,
  table.header([*Snapshot*], [*All Rust*], [*app.rs*], [*app family*]),
  [`27f644c`: before architecture campaign], [85,253], [14,130], [32,745],
  [`d3ca988`: first postmortem snapshot], [93,942], [13,980], [34,237],
  [`8cd06ea`: step 1 commit], [94,362], [12,840], [34,506],
  [`acafbcd`: step 2 commit], [94,579], [12,647], [34,723],
  [`74cf297`: steps 3–4 commit], [94,831], [12,400], [34,920],
  [Current worktree], [95,047], [12,341], [34,890],
)

From the original architecture baseline, total Rust remains *9,794 lines larger
(11.5%)*, while the root is *1,789 lines smaller (12.7%)*. From the first
postmortem, the root shrank but its family grew. Moving functions out of the
root cannot, by itself, reduce the family's total.

These commit intervals are not pure refactoring experiments. In particular,
the step 1 commit includes build identity and window/Settings/folder fixes.
Todo 236 records an immediate pre-extraction root of 14,033, whereas `d3ca988`
contains 13,980. Its local −1,193 figure and this table's −1,140 commit delta
use different starting points; neither should be substituted for the other.

== Correcting the earlier test estimate

The first report counted from an inline test-module declaration to the end of
its file. That overcounts tests when production code follows the module.
`src/app/settings_window.rs` is a concrete example: its tests end at line 411,
but the file continues to line 846. Therefore the older “non-test remainder”
was understated. This report stops at the module's unindented closing brace.

#table(
  columns: (2fr, 1fr, 1fr, 1fr), inset: 5pt,
  table.header([*Category*], [*First postmortem*], [*Now*], [*Delta*]),
  [Identified test lines], [32,011], [32,813], [+802],
  [Other Rust estimate], [61,931], [62,234], [+303],
  [Exact total], [93,942], [95,047], [+1,105],
)

This is still a source-layout approximation, not compiler-derived production
LOC. Test files follow the same naming convention as before; inline tests use
formatted top-level `cfg(test)` / `mod tests` blocks. Scattered test-only helpers,
QA code, comments and tooling remain in “other.” Do not call it executable code
or use it to infer RAM. The corrected app-family remainder rises from 24,245
to 24,354: +109. The exact family total rises by 653.

== Did the five steps meet their own promises?

- *Leaf extraction:* yes for organization; no total-size saving was promised.
  Todo 236 records +69 lines of module/import/formatting overhead locally.
- *Navigation:* introduced a useful focus intent and common handoff, but added
  36 non-test lines at that step. Native WebView first-responder acceptance is
  still open in item 237. It is not honest to call all five steps fully accepted.
- *Explorer boundary:* actual restriction of view access, not just a file move.
  It cost +91 non-test lines at introduction. The later cleanup removed parallel
  output locals, needless generic output and repeated empty-state rendering.
- *Save integration:* achieved a small −9-line production consolidation while
  retaining receipts and safety gates. The subsequent worker-failure bug shows
  completion receipts were not the only error path needing identity checks.
- *Preview:* consolidated admission/visibility policy and native teardown, but
  cost +22 production lines to address invalid endpoints and suspended state.
  That is a correctness tradeoff, not a size win.

These figures are local task accounting, not an additive reconstruction of the
commit table. Tests and intervening repairs have separate costs. The work bought
clearer boundaries and some safety, but did not satisfy the earlier aspiration
of a root below about 10,000 lines or a smaller overall implementation.

= Why the app is still enormous

== The coordinator still owns nearly everything

`EditorApp` declares *128 named fields*: 129 at the first postmortem and 128
before the original architecture campaign. This includes platform-conditional
declarations, not the simultaneous runtime layout. One fewer field is not a
meaningful decentralization of ownership.

It still coordinates documents/tabs, saves, workspace and indexing, compilation,
Tinymist synchronization, native preview views, hover/completion, menus and
shortcuts, Settings propagation, dialogs, history and status. Ten child modules
still contain `impl EditorApp` blocks: editor, extra shortcuts, Git actions,
miTeX mode, native views, navigation, saves, Settings view, tabs and workspace.
Moving those methods did not remove their access to the full app object.

That is not an argument for making ten new controllers. Some integration must
exist. It is an argument for removing duplicated decisions before adding another
owner, request type, cache or effect adapter.

== Root hotspots remain almost unchanged

Measured function spans include signatures, comments and blanks:

#table(
  columns: (2.2fr, 0.7fr, 2.8fr), inset: 5pt,
  table.header([*Function*], [*Lines*], [*Interpretation*]),
  [`handle_shortcuts`], [296], [Same size as first report; focus and event ordering are real constraints.],
  [`ui_in_window`], [288], [Same size; still sequences the whole frame.],
  [`new_session`], [288], [Still initializes an app-wide collection of state.],
  [`receive_tinymist_events`], [225], [Down from 250, but still connects many state owners.],
  [`restart_tinymist_with_handoff`], [183], [Down from 186; restart orchestration barely shrank.],
  [`show_toolbar`], [194], [Down from 197; menus, tabs and controls remain here.],
  [`show_workspace`], [73], [Down from 331; the clearest successful narrowing.],
)

There are another *3,985 lines* from `request_pdf_pages` at line 8,357 through
EOF: predominantly free helpers and supporting types. These include PDF
presentation, icons, URL parsing, Settings controls, commands, text transforms
and table editing. They are not a giant inline test suite. Moving tests out is
not the answer: `src/app/tests.rs` already has 7,557 lines.

= What to do differently now

== Start with deletions whose evidence already exists

The following are proposed changes, not implemented fixes or guaranteed savings.
Review behavior first; reject a candidate if its abstraction costs more than
the duplication it removes. Do one at a time.

=== First: share open-dialog configuration, not dialog lifetimes

`open_in_new_window_dialog` and `start_open_dialog` repeat supported extensions,
Typst/PDF filters, initial directory, admission and request construction.
The second also offers an Images filter; titles and request targets differ.
Extract the common dialog configuration using ordinary Rust functions or
constants, leaving the destination and ownership explicit. Do not build a
general dialog framework or merge Settings-parent behavior with document-parent
behavior: those native parents deliberately differ.

Acceptance: delete the repeated filter/configuration block; preserve both titles,
the Images-filter distinction, file-or-folder selection, window parent and
document key. Require a net-negative non-test diff. The expected opportunity is
small (tens of lines), not thousands. Test configuration and destination routing;
observe native parenting if that implementation changes.

=== Second: consolidate identical actions across command entry points

Inspect `handle_shortcuts`, `execute_app_command`, menu dispatch and
`app/extra_shortcuts.rs` together. Move only duplicated action effects into the
existing command execution path. Keep input admission separate: child focus,
text-field focus, completion consumption and Cmd+Shift+W precedence cannot be
flattened into a single unconditional action table.

Acceptance: list the duplicated branches actually removed, preserve enabled-state
and focused-owner behavior, and cover menu/key equivalence plus child text input.
Do not add an event bus, heap-allocated command closures or a per-frame command
registry. If there are no identical branches, stop: file length alone is not
evidence of redundancy. Savings need an implementation spike, not a promise.

=== Third: remove repeated transition decisions, not necessary identity

Review Tinymist restart/receive paths alongside `PreviewController` and the sync
coordinator. Current compile-status admission checks both synchronization
generation and preview recovery generation. These may protect different owners;
they are not automatically duplicate just because both mention generation.
Inventory writers and lifecycle differences before deleting a condition.

Use the existing controller for a policy only when it can own the whole decision.
Do not copy the same flags into another state model and then synchronize both.
Keep the newly added save continuation token: a worker failure has no receipt,
so retaining request identity there is necessary information, not removable bloat.

Acceptance: remove a named redundant decision/state writer, with stale generation,
LSP-only, replacement-preview, suspension and matching/newer-save-failure tests.
The prior audit missed two edge paths; passing component tests alone is not
enough. This is higher risk and comes after the small consolidation work.

== Separately: make app.rs smaller without pretending total LOC falls

A cohesive move of Settings-only helpers/types to existing Settings modules is
the clearest readability option. Much of the range around lines 10,238–10,904
contains font pickers, overrides and status controls; shortcut-editor helpers
also remain near EOF. First identify all callers: some controls are shared with
document popups. Move each helper to its real owner rather than create a helper
dumping ground or a module cycle through root re-exports.

Icons/geometry and preview URL/navigation utilities are other cohesive families.
These moves can substantially lower the root's line count while leaving the
total near-neutral. They should have a separate budget and acceptance label:
*readability*, not *code reduction* or *performance*. No additional controller
is required just to relocate pure functions.

Getting below 10,000 root lines would require removing or relocating another
2,342 lines. It is plausible, but the inspected Settings block alone does not
deliver it. A sub-5,000-line root is not a safe near-term promise. Nor does either
number prove fewer decisions or a faster app.

== Do not manufacture a win

Keep stable tab/document identity, durable save gates, bounded index workers,
shared workspace observation and demand-driven PDF residency. These address
specific correctness/resource problems. Do not delete their checks or tests to
make a diff look negative. Do not compress readable branches onto one line,
replace them with opaque macros, or defer work into a dependency without counting
the new dependency and behavior cost.

Larger savings may require removing product features or supporting fewer modes.
That is a user decision, not authorized cleanup. There is presently no measured
duplicate-code inventory supporting a safe multi-thousand-line deletion target
while preserving every feature. We should say that rather than promise one.

= A better acceptance contract

For each proposed patch, record before editing:

1. *Purpose:* deletion, ownership change, relocation or bug fix. Do not mix these
   labels to obscure growth.
2. *Deletion:* exact duplicated branches, state or implementation being removed.
   A deletion task needs a net-negative non-test result; otherwise reconsider it.
3. *Accounting:* root, affected module family, all Rust, identified tests and
   remaining Rust, against one named commit or saved pre-edit snapshot.
4. *Behavior:* targeted regression and integration coverage. Preserve the open
   native-focus acceptance in todo 237 instead of checking it off by inference.
5. *Resources:* retain worker/cache bounds and no-idle-repaint invariants. No
   material performance impact is expected from static helper relocation.
   Measure matched optimized workloads if scheduling, hot-loop work or ownership
   changes; LOC is neither CPU time nor GPU memory.

Stop after each patch and compare the promised deletion with the actual diff.
If two supposed simplifications instead add glue, change the plan rather than
keep executing the same extraction strategy. Small well-supported savings are
preferable to a large new abstraction advertised as future simplification.

= Evidence and limitations

This report changes no application code. It inspected current source, Git
snapshots, `postmortem.typ`, todo 236–242 and the preceding cleanup evidence.
The preceding cleanup recorded 1,072 passing full-suite tests and 13 xtask tests;
those were not rerun for this documentation-only task. No new native interaction
or performance measurement is claimed. Compilation of this Typst source checks
syntax, not native UI acceptance or the report's visual layout.

Reproduce exact totals using `git ls-tree -r --name-only REF`, filter `.rs`,
then sum line counts from `git show REF:PATH`. For the worktree, use tracked
paths from `git ls-files` and read current contents. There are 144 tracked Rust
files now versus 138 at `d3ca988`; no untracked Rust file was omitted at review.
For the test approximation, count complete conventionally named test files;
otherwise count only each top-level `cfg(test)` / `mod tests` block through its
own closing brace. This relies on the repository's rustfmt layout, not an AST.

Useful verification commands:
```sh
git diff --numstat d3ca988 -- '*.rs'
git show d3ca988:src/app.rs | wc -l
wc -l src/app.rs src/app/*.rs
rg -n 'impl EditorApp' src/app
```

*Bottom line:* the organization improved, but the code-reduction objective was
not achieved. The app is still huge because the coordinator and its helpers
remain broad, while extractions added interfaces and bug fixes added safeguards.
Keep the defensible safeguards. Replace the next extraction campaign with
deletion-first work, and treat root-file housekeeping as a separate outcome.
