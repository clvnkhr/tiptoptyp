# Activity panel

Open the bottom panel and select **Activity**, after Problems and Terminal.
Compact status chips wrap horizontally and use the same remembered panel height
as Problems and Terminal. Hover a chip for its role, precise
pending state, or failure reason. Maximize/restore and close work like the other
tabs. Labels stay fixed when states change; colour carries the state. The
Activity tab tooltip explains the colours, and each source tooltip explains its
purpose and current state.

- Green: healthy, up to date, or idle with no requested work.
- Yellow: stale, queued/debouncing, starting, or working.
- Red: an operation or service failed; hover for the reason.
- Grey: disabled, paused, not started, or not applicable to this document.

An error or warning *in the document* does not itself mean the diagnostic
service failed. A completed diagnostic pass is green. A failed PDF build is
red because it could not produce the requested artifact.

## Sources

| Indicator | Work observed |
| --- | --- |
| Save | Autosave debounce, explicit saves, write conflicts and durability failures |
| Typst | CLI PDF builds, independently of the live preview |
| Tectonic | TeX PDF builds, including automatic recompilation |
| Tinymist | Startup, current diagnostics, hover, completion and formatting requests |
| Live preview | Tinymist interactive compilation |
| Typst diagnostics | Freshness of the active document's editor errors/warnings |
| TexLab | Startup, hover/completion requests and diagnostics |
| Badness | Startup, lint diagnostics and formatting |
| Tinymist format / Badness format / tex-fmt | The selected formatter's explicit requests and failures |
| Compiler diagnostics | CLI build-result freshness and build failures |
| Vector view | Tinymist's live preview surface |
| PDFium preview | Compiled PDF pages when the PDFium preview is selected |
| Opened PDF | Display and text search for a PDF opened as a document |
| File loading | Opening a PDF or decoding an image |
| Hover thumbnail | Small image/PDF previews shown on hover |
| Git status | Repository scans and repository operations |
| Git hunks | Debounced changes against the current editor buffer |
| Git mutation | Applying selected hunk/file changes |
| Workspace | Coalesced filesystem scans |
| File watcher | Filesystem notification service health, independent of scans |
| Project index | Debounced project symbols/references/outline work |
| Harper | English spelling/grammar checks |
| Unicode | Suspicious-character checks (shares the writing-check worker) |
| Fonts | Workspace font scans |
| Packages | Package catalog loading, including registry failures |
| Package removal | Package deletion operations |
| File import | Queued/running file imports |
| Terminal | Shell startup, connection, exit and failure |

This is work/freshness telemetry, not a CPU sampler. Terminal green means a
healthy connected session; the app does not infer whether an arbitrary shell
command is busy. Synchronous editor drawing/parsing completes within its frame
and has no separate persistent busy state. Language-server notifications that
omit versions follow the existing accepted-notification policy; exact versioned
results are checked against document ownership, epoch and revision. The future
standard LaTeX adapter remains unimplemented.

## Ownership and performance

Each subsystem owns its state. Activity observes existing deadlines, service
states, accepted results and jobs; painting cannot start, restart, or poll a job.
Latest/exclusive workers retain their last failure until retried or superseded.
Worker panics become a failed result and wake the owner. Workspace scan events
and terminal lifecycle changes wake their existing owner; ordinary hidden
terminal output remains silent. There is no Activity timer, animation, event
history, disk logging, or process spawning. Status rows are assembled only while
Activity is visible; storage/work is bounded by the fixed source inventory.
No throughput or startup improvement is claimed.

Deterministic tests cover freshness, retained failures/recovery, worker panics,
compact wrapping at 320/800/1400 px, unchanged label positions across states, semantic tab switching and hidden terminal
output versus lifecycle notifications. The `activity-panel` snapshot scene
supplies mixed states without launching tools to simulate failures.

[Inspect the captured Activity panel](activity-panel.png). This is a fresh native
viewport framebuffer with deterministic example states, not a live process
sample or a composed desktop screenshot.

Validation (2026-09-24): full Rust suite, 1,076 application unit tests,
15 tooling tests, formatting and strict all-target Clippy passed. The release
Activity framebuffer was freshly captured and inspected; the maintained
23-image gallery was regenerated in one session and decoded successfully.
