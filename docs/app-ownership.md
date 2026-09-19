# App ownership and next boundaries

Current map for todos 246–254, inspected at `722763a` (20 September 2026).
Update this map when an owner changes. Native acceptance remains open in todos
229 (hover), 234 (Settings flash) and 237 (preview-to-editor keyboard focus).

## Ownership and effects

| State / existing owner | Lifetime and writers | Boundary to preserve |
| --- | --- | --- |
| `Tabs` / `TabRecord` / `DocumentSession` | Document: source, revision/epoch, projection, folding, parked editor state, autosave and workspace. `tabs.rs` activates/rekeys records; edits go through document APIs. | Stable tab ID is distinct from a document lifetime and the preview entry. Keep undo/selection in its owning viewport. |
| `DocumentLifecycle`, `DocumentWorkflow`, `PendingSave` | Window: new/open/close, dialogs, continuation tokens, protected saves. Root app, `tabs.rs`, `saves.rs`, `workspace_view.rs` apply transitions. | Save receipt admission and durability remain in existing workflow/core APIs; disk work stays in `save_io` under its path lease. |
| `Compiler`, `TinymistSidecar`, sync `Coordinator` | Window services: root request/restart/event adapters, `mitex_mode.rs`, tabs and saves supply versioned input. | Coordinator owns open URIs/backings; services own processes; adapters check result identity before document/UI effects. No service per tab. |
| `PreviewController` (designated and asset), page/asset loaders | Window: compile deadlines/pause, artifact, status, recovery, visible page demands and asset tokens. Root, raster/native/workspace views request work and accept results. | Controller owns readiness/recovery/visibility policy. PDF service and process-wide residency own decoding and byte budgets. |
| `webview*`, native parent, browser channels/job | Window, UI thread: `native_views.rs` creates/applies/hides/discards; root restart/event adapters request reload, navigation reclaims focus. | `discard_webview` already owns teardown. Keep native resource lifetime separate from server generation and readiness. Browser launches stay bounded/off the UI thread. |
| `EditorDerivedData`, highlighters, pair syntax | Window caches keyed to active document identity: root preparation and `editor_view.rs` consume them. | Invalidate by source/key/theme; no reparsing on pointer movement or ordinary unchanged frames. |
| Find flags/query/replacement, `SearchSession` | Window presentation over active document: `editor_view.rs`, root open/close/Escape, `extra_shortcuts.rs`; tab/open/empty transitions clear cached results. | Search owns results/cache; edits use `DocumentSession`; navigation distinguishes Search from Focus. |
| Editor hover/completion/caret, request tokens, format keys | Requests belong to a document key within a window. Root request/event/key handlers, editor drawing and tab/lifecycle resets write them. | Reject obsolete document/version/generation/token results; completion uses the existing one-undo transaction. |
| Tooltip request, diagnostic/asset overlays, app popup, rename/table dialogs | Window/child: root dismissal and painting, `tooltips`, `native_views`, `ChildViewHost`, and Git actions. | Child generations suppress late callbacks; distinct semantic/control/asset policies must not erase each other's requests. |
| `ExplorerPanelState`, `WorkspaceTree`, index/debounce/client, root/history | Window selection/presentation; immutable canonical-root observation is shared by `WorkspaceClient`. Root/workspace adapters apply `explorer_view` outputs after paint. | View has borrowed inputs; workspace/index services own observation, bounded execution and result routing. |
| `GitPanel`, `GitEditorState`, hunk job | Window views/cache, root and `git_actions.rs` dispatch; repository service executes. | Panel and hunk mutations share the repository-root lease and revalidate under it. |
| `AppSettings`, pending settings, presentation/fonts/theme | Per-window applied snapshot; `AppShell` merges/broadcasts process preferences. Root/settings adapters queue changes and apply only changed presentation. | `SettingsWindow` is retained by the root; secondaries request it. Font catalog jobs/caches keep their existing revision/owner bounds. |
| Shortcut query/capture/notice, overrides visibility, pending paste | Window/child presentation: root shortcut handler, `settings_view`, `extra_shortcuts`; focused Settings queues native edit events. | Capture consumes input before commands; widget edits must not mutate the document behind them. |
| Package query/filter/catalog/job, tool preferences/resolutions/capabilities | Window: package view emits actions; root/settings adapters perform jobs and refresh cached tool state. | No network/tool discovery in paint; keep service work outside leaf views. |
| View mode, Problems, notices/status log/title | Window: command handler, toolbar and document/preview event adapters. | Commands should have one effect implementation; status history stays bounded. |
| Window requests, native menu queue, open requests, host/close state | `AppShell` owns process/window routing; each `EditorApp` consumes commands in its viewport. | No-document policy belongs to shell; empty workspace policy belongs to editor. Settings is the root-owned exception. |
| Captures, scene and `QaSession` | Opt-in window/test state. | Preserve non-persistence and bounded capture/profile behavior. |

The child modules `editor_view`, `extra_shortcuts`, `git_actions`, `mitex_mode`,
`native_views`, `navigation`, `saves`, `settings_view`, `tabs` and `workspace_view`
still implement `EditorApp` and can access all its fields. Moving methods among
them does not narrow ownership. Existing leaf boundaries include `explorer_view`,
`completion_popup`, `package_browser`, `popup_layout`, icons and Settings controls.

## Reviewed candidates

| Todo | Exact callers / repeated work | Proposed scope and verification |
| --- | --- | --- |
| 248 | `show_toolbar` writes Problems, Explorer, Find and view mode; `execute_app_command` repeats those effects. `handle_shortcuts` and `process_native_menu_commands` already delegate. `execute_extra_shortcut` has distinct search/fold/raster policies. | Route the shared toolbar commands through existing execution; fold the three identical view-mode controls into a fixed array loop. Retain labels, IDs, selection and layout. Exercise actual toolbar clicks, menu queue and keys, enabled state, owner and one-shot toggles. Do not create a second dispatcher or force extra actions into `AppCommand`. |
| 249 | `show_shortcut_editor_window` passes query/capture/notice into already-free `show_shortcut_editor_contents`; root `handle_shortcut_capture` and extra shortcut open/close write the same presentation state. Contents clones all settings on each paint. | A small shortcut-editor state and borrowed settings/action view is justified; use the existing Settings modules. Test capture/cancel/conflict/reset, unchanged-frame no clone/recompute, and singleton broadcasts. Merely relocating the free renderer is insufficient. |
| 250 | `show_find_bar`, `apply_find_actions`, `open_find`/`toggle_find`, Escape and extra shortcuts repeat close/case/regex/cache transitions. | Borrow source/key and `SearchSession`, own presentation flags/query, return edit/navigation intents. Test search-focus retention, Unicode one-undo replacement, stale tab identity and unchanged cache reuse. Preserve `EditorSelection` intent; native acceptance still needs 237. |
| 251 | `receive_formatted_document` already calls `prepare_canonical_edits`; completion calls its existing transaction; both use document mutation APIs. Formatting additionally maps selection and coordinates manual-save follow-up. | Defer a shared formatting/completion transaction: inspection found no duplicated validation algorithm to remove. Retain the projected-formatting one-undo regression and reply-identity tests. Revisit only a specifically duplicated adapter step; do not weaken formatting/save ordering to satisfy this candidate. |
| 252 | `update_editor_hover`, `dismiss_hover_on_scroll`, `dismiss_keyboard_tooltip`, `clear_preview_for_document`, `activate_record` and child lifecycle cleanup can end hover. | Inventory same-lifetime writers first, then one semantic-hover invalidation path, conditional on 229's native reproduction. Test old dismissal versus newer popup, triangle handoff, scroll, tab switch and zero unchanged-source reparsing. Asset/control policies remain separate. |
| 253 | `update_webview`, `hide_webview`, `discard_webview` own handle/cache/navigation; root restart and Ready handling still write reload intent. | Defer a new resource wrapper: teardown/property diff already have single owners (240). Investigate whether root reload writes can produce an invalid lifecycle in the native reproduction before proposing another abstraction. Existing property-diff/teardown/URL-reuse tests are the baseline. |
| 254 | `reset_untitled_document`, `load_path` and `activate_record`/`activate_tab` clear selection/attention/search and request service changes; they differ on rekeying, loaded bytes and pinned preview. | Limit the first batch to a shared transient-editor reset if caller ordering can be preserved; leave service-restart decisions explicit. Test dirty/cancel, parked Save As, empty/folder workspace and repeated New with unchanged pinned preview generation. |

Search the named symbols when reading this map; line numbers intentionally do not
serve as dependencies. Existing tests worth retaining include
`navigation_entry_points_focus_only_their_owner_and_keep_mac_arrow_shortcuts`,
`projected_application_formatting_maps_source_and_cursor_then_undoes_one_edit`,
`replacement_preview_server_forces_navigation_even_when_its_url_is_reused`, and
the save/tab/tooltip suites. Native helper tests do not close 229/234/237.

## Baseline and counting

At `722763a`, before 246–248:

| Scope | Physical Rust lines | Identified tests | Remainder |
| --- | ---: | ---: | ---: |
| `src/app.rs` | 11,318 | 0 | 11,318 |
| Root app + `src/app/**` | 34,909 | 10,633 | 24,276 |
| All tracked Rust | 95,124 | 32,942 | 62,182 |

Root `EditorApp` has 128 field declarations. Candidate module sizes (total /
identified tests / remainder): `extra_shortcuts.rs` 427/175/252;
`editor_view.rs` 1253/0/1253; `settings_view.rs` 465/0/465;
`native_views.rs` 1528/52/1476; `tabs.rs` 1192/0/1192;
`lifecycle.rs` 82/40/42. The root `tests.rs` is 7,581 lines.

Count tracked `.rs` paths with `git ls-files '*.rs'` and physical file lines;
include new source files when measuring the working tree. Test estimates include
files under `tests/`, `tests.rs`, `*_tests.rs`, and actual `#[cfg(test)] mod`
blocks up to their matching rustfmt-indented closing brace. Count no suffix after
that brace: `settings_window.rs` has production below its tests. Remainder includes
comments, tooling and scattered test-only helpers, so it is not executable LOC.
Totals differ from the older walkthrough because its revision and test
classification are different; compare this batch with this baseline.

## 246–248 result

The toolbar now sends Find, Settings, Explorer, Problems and all three view modes
through `execute_app_command`. `show_titlebar_menus` replaces the three repeated
File/Edit/View button implementations. Keyboard/menu command execution and the
distinct extra-shortcut policies retain their existing owners.

The expanded availability matrix reproduced a mismatch: the empty workspace
enabled Problems and view modes on the toolbar despite rejecting those commands.
The toolbar now uses the existing admission checks. The old document-kind helper
and its eight-line unit test are replaced by actual control availability tests
for empty, Typst, text, PDF and image workspaces.

| Checkpoint | Root app | App family total / tests / remainder | All Rust total / tests / remainder |
| --- | ---: | ---: | ---: |
| 246, inventory only | 11,318 | 34,909 / 10,633 / 24,276 | 95,124 / 32,942 / 62,182 |
| 247, five characterization tests passing before consolidation | 11,320 | 35,287 / 11,009 / 24,278 | 95,502 / 33,318 / 62,184 |
| 248, final controls and expanded regression matrix | 11,308 | 35,327 / 11,061 / 24,266 | 95,542 / 33,370 / 62,172 |

247 adds 376 test-file lines and two test-module registration lines (counted in
the remainder). 248 removes 12 production lines and adds 52 net test lines,
including compact layout coverage and replacement of the old helper test.
Across all three items, total Rust grows 418 lines: identified tests +428 and
remainder −10. The 128 root field declarations are unchanged; this first batch
consolidates decisions, while later justified extractions address access.

`src/app/command_tests.rs` contains six deterministic tests, including 56
owner/route/control cases (seven commands, two owners, four routes), 40
busy/empty/document-kind admission cases, focus/capture/completion priority,
menu switching, and actual disabled controls. Existing shell and Settings tests
cover native-command delivery to the selected owner, one singleton Settings
viewport, child text editing and the macOS no-window command policy. The first
five characterization tests passed before production edits. The empty-workspace
control test failed before its admission fix and passed afterward.

Performance impact is limited to three-element stack arrays and existing command
dispatch on clicks. Button construction/order, borrowed static labels and idle
behavior are preserved. No new worker, retained cache, source copy, disk work or
repaint schedule is introduced. This batch makes no timing or native composition
claim; the existing native acceptance items remain open. Validation details are
in the 246–248 work diary at the bottom of `todo.typ`.
