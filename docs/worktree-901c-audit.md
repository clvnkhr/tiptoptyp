# Audit of worktree 901c (24 September 2026)

Audited the uncommitted changes based on `481f340` against master `fcfaedf`.
The original work is preserved in commit `f4abb72` on `codex/archive-901c`;
its worktree can be removed without losing the rejected proposals.

| Area | Disposition |
| --- | --- |
| Shared document/popup window policy and its callers | Already integrated in master; `window_policy.rs` is byte-for-byte identical. Keep the newer lifecycle, native ownership, alpha, and focus fixes. |
| Typst source allocation on highlight cache lookup | Retained the borrowed `Cow` path, avoiding a full source copy. |
| Revision-only highlight cache | Rejected. The editor captures the document key before `DocumentSession::edit`; TextEdit invokes its layouter on changed text before the transaction advances that key. Text comparison must remain to prevent stale layout while typing. Added an in-transaction edit regression. |
| Generic syntax extension normalization | Retained: compare extensions without allocating; lowercase only on cache misses. Case changes preserve the highlighting, and unknown/no-extension behavior remains covered. |
| Replace-all match copying | Retained: move the completed matches and restore them on no-op replacements. Added a no-op cache regression. |
| Replacement change detection | Retained: compare document keys after the edit transaction, avoiding an extra document copy. This is safe after transaction commit, unlike the proposed in-layouter cache shortcut. |
| Unused path cloning | Retained: prepare paths only for the editor modes that use them. |
| Borrowed search-highlight result slice | Not imported. The proposal holds the mutable `SearchSession::results` borrow while calling `selected()` and subsequent mutable editor preparation. It needs a separate snapshot/lifetime design; keep the current owned ranges. |
| Cached image visibility | Replaced with iterator forwarding to remove the intermediate vector. Do not suppress calls to the residency budget: these calls update recency, which controls eviction across windows. |

No native geometry, window policy, or visual contract changes are made by this
integration, so deterministic tests are sufficient; no screenshot gallery is
regenerated. No repaint loop, worker, or instrumentation is added.

## Performance evidence

`scripts/probes/editor-allocation.rs` compares the removed allocation primitives
and their replacements in one optimized (`rustc -O`) executable. Each sample
uses 100 warmups and 2,000 measured operations, a 1,120,000-byte Typst-like string,
and 10,000 match ranges. It retains full text equality checks on both source paths.
Both variants use identical input, hardware, and optimization settings. This is a
headless cache-hit/copy microbenchmark, not parser, layout, cold startup, active
editing, or whole-app latency. The match test isolates clone versus move/restore,
not the rest of replace-all. No viewport/theme applies. Timings establish neither
cross-platform performance nor UI responsiveness.

Recorded samples and environment: [allocation probe metadata](performance-results/901c-allocation-audit-2026-09-24.json).
Median source copy/comparison: 99,670 ns;
borrow/comparison: 61,553 ns.
These figures measure only the described primitives, not the full highlighter.

Validation: formatting and strict Clippy passed; the complete test suite passed
(1235 tests), including all three new regressions. All 15 xtask tests passed.
The old worktree was removed after saving its original patch in Git.
